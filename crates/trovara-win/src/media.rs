//! Audio and video through Media Foundation (TRD §6.5): Windows' own decoders (MP3, AAC/M4A, WAV, FLAC, WMA, ALAC,
//! Opus/Vorbis where installed; H.264, HEVC, VP9, AV1, MPEG-2, WMV in MP4, MOV, MKV, AVI, WMV, TS…). Nothing to ship.
//!
//! - `audio`: the soundtrack as 16 kHz mono float, handed over a block at a time (memory stays small for a 3-hour
//!   podcast). Media Foundation's resampler does the conversion when it can; otherwise the native rate is decoded and
//!   resampled here.
//! - `frames`: pictures at chosen times, by seeking. A seek lands on the keyframe before the time; when that's more
//!   than a step back (long GOPs: screen recordings), decoding goes forward to the time.

use std::path::Path;
use std::sync::Once;

use trovara_core::image::RgbImage;
use windows::core::{Interface, GUID, HSTRING};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

pub const SAMPLE_RATE: u32 = 16_000;
/// 100-ns units per second (Media Foundation's clock).
const UNITS: f64 = 10_000_000.0;

const FIRST_VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const FIRST_AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const ALL_STREAMS: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
const MEDIA_SOURCE: u32 = MF_SOURCE_READER_MEDIASOURCE.0 as u32;

fn startup() -> windows::core::Result<()> {
    static START: Once = Once::new();
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let mut result = Ok(());
    START.call_once(|| result = unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) });
    result
}

fn reader(path: &Path, video_processing: bool) -> windows::core::Result<IMFSourceReader> {
    startup()?;
    unsafe {
        let mut attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attributes, 2)?;
        let attributes = attributes.unwrap();
        if video_processing {
            // Colour conversion (YUV → RGB32) and scaling inside the reader.
            attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
        }
        attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attributes)
    }
}

/// What a media file holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub duration: f64,
    pub audio: bool,
    pub video: bool,
    /// The video's size as stored.
    pub width: usize,
    pub height: usize,
}

fn has_stream(reader: &IMFSourceReader, stream: u32) -> bool {
    unsafe { reader.GetNativeMediaType(stream, 0).is_ok() }
}

fn duration(reader: &IMFSourceReader) -> f64 {
    unsafe { reader.GetPresentationAttribute(MEDIA_SOURCE, &MF_PD_DURATION) }
        .ok()
        .and_then(|value| u64::try_from(&value).ok())
        .map_or(0.0, |units| units as f64 / UNITS)
}

fn frame_size(media_type: &IMFMediaType) -> Option<(usize, usize)> {
    let packed = unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }.ok()?;
    Some(((packed >> 32) as usize, (packed & 0xFFFF_FFFF) as usize))
}

pub fn info(path: &Path) -> Result<Info, String> {
    let reader = reader(path, false).map_err(|e| unreadable(&e))?;
    let mut info = Info {
        duration: duration(&reader),
        audio: has_stream(&reader, FIRST_AUDIO),
        video: has_stream(&reader, FIRST_VIDEO),
        ..Info::default()
    };
    if info.video {
        if let Some((w, h)) = unsafe { reader.GetNativeMediaType(FIRST_VIDEO, 0) }
            .ok()
            .and_then(|t| frame_size(&t))
        {
            (info.width, info.height) = (w, h);
        }
    }
    Ok(info)
}

fn unreadable(error: &windows::core::Error) -> String {
    let code = error.code().0 as u32;
    match code {
        0xC00D36C4 => "Windows has no decoder for this file's format (MF_E_UNSUPPORTED_BYTESTREAM_TYPE)".into(),
        0xC00D5212 => "Windows has no decoder for this file's codec (install its Microsoft Store extension, or the optional ffmpeg pack)".into(),
        _ => format!("can't read media ({})", error.message()),
    }
}

/// Decodes the soundtrack to 16 kHz mono float and hands it to `each` in blocks, in order. Returns false when the
/// file has no audio.
pub fn audio(
    path: &Path,
    mut each: impl FnMut(&[f32]) -> Result<(), String>,
) -> Result<bool, String> {
    let reader = reader(path, false).map_err(|e| unreadable(&e))?;
    if !has_stream(&reader, FIRST_AUDIO) {
        return Ok(false);
    }
    unsafe {
        reader
            .SetStreamSelection(ALL_STREAMS, false)
            .map_err(|e| unreadable(&e))?;
        reader
            .SetStreamSelection(FIRST_AUDIO, true)
            .map_err(|e| unreadable(&e))?;
        // Ask for 16 kHz mono float. Where the reader can't convert, take float at the native rate and resample here.
        let (rate, channels) = match set_float(&reader, Some((SAMPLE_RATE, 1))) {
            Ok(()) => current_format(&reader).unwrap_or((SAMPLE_RATE, 1)),
            Err(_) => {
                set_float(&reader, None).map_err(|e| unreadable(&e))?;
                current_format(&reader).ok_or("no audio format")?
            }
        };
        let mut resampler = (rate != SAMPLE_RATE).then(|| Resampler::new(rate, SAMPLE_RATE));
        loop {
            let mut flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            reader
                .ReadSample(
                    FIRST_AUDIO,
                    0,
                    None,
                    Some(&mut flags),
                    None,
                    Some(&mut sample),
                )
                .map_err(|e| unreadable(&e))?;
            if let Some(sample) = sample {
                let floats = sample_floats(&sample).map_err(|e| unreadable(&e))?;
                let mono = downmix(&floats, channels as usize);
                match resampler.as_mut() {
                    Some(r) => each(&r.process(&mono))?,
                    None => each(&mono)?,
                }
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        if let Some(r) = resampler.as_mut() {
            each(&r.finish())?;
        }
    }
    Ok(true)
}

unsafe fn set_float(
    reader: &IMFSourceReader,
    format: Option<(u32, u32)>,
) -> windows::core::Result<()> {
    let media_type = MFCreateMediaType()?;
    media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
    media_type.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)?;
    if let Some((rate, channels)) = format {
        media_type.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
        media_type.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)?;
        media_type.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 32)?;
        media_type.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4 * channels)?;
        media_type.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 4 * channels * rate)?;
    }
    reader.SetCurrentMediaType(FIRST_AUDIO, None, &media_type)
}

unsafe fn current_format(reader: &IMFSourceReader) -> Option<(u32, u32)> {
    let media_type = reader.GetCurrentMediaType(FIRST_AUDIO).ok()?;
    Some((
        media_type.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).ok()?,
        media_type.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).ok()?,
    ))
}

unsafe fn sample_floats(sample: &IMFSample) -> windows::core::Result<Vec<f32>> {
    let buffer = sample.ConvertToContiguousBuffer()?;
    let mut data = std::ptr::null_mut();
    let mut length = 0u32;
    buffer.Lock(&mut data, None, Some(&mut length))?;
    let floats = std::slice::from_raw_parts(data as *const f32, length as usize / 4).to_vec();
    buffer.Unlock()?;
    Ok(floats)
}

fn downmix(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// A windowed-sinc resampler (Blackman window, 64 taps a side at the lower rate's cutoff), streaming: for the rare
/// file whose rate Media Foundation won't convert.
pub struct Resampler {
    ratio: f64,
    cutoff: f64,
    history: Vec<f32>,
    /// The next output's position in input samples, counted from the start of `history`.
    position: f64,
}

const TAPS: usize = 64;

impl Resampler {
    pub fn new(from: u32, to: u32) -> Resampler {
        let ratio = from as f64 / to as f64;
        Resampler {
            ratio,
            cutoff: (1.0 / ratio).min(1.0) * 0.95,
            history: vec![0.0; TAPS],
            position: TAPS as f64,
        }
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.history.extend_from_slice(input);
        let mut out = Vec::with_capacity((input.len() as f64 / self.ratio) as usize + 1);
        while self.position + (TAPS as f64) < self.history.len() as f64 {
            out.push(self.at(self.position));
            self.position += self.ratio;
        }
        // Keep what the next outputs still need.
        let keep_from = (self.position as usize).saturating_sub(TAPS);
        self.history.drain(..keep_from);
        self.position -= keep_from as f64;
        out
    }

    pub fn finish(&mut self) -> Vec<f32> {
        self.process(&vec![0.0; TAPS])
    }

    fn at(&self, position: f64) -> f32 {
        let center = position.floor() as isize;
        let (mut sum, mut weights) = (0.0f64, 0.0f64);
        for i in -(TAPS as isize) + 1..TAPS as isize {
            let index = center + i;
            if index < 0 || index as usize >= self.history.len() {
                continue;
            }
            let x = index as f64 - position;
            let sinc = if x == 0.0 {
                1.0
            } else {
                (std::f64::consts::PI * x * self.cutoff).sin()
                    / (std::f64::consts::PI * x * self.cutoff)
            };
            let t = (x / TAPS as f64 + 1.0) / 2.0; // 0…1 across the window
            let window = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * t).cos()
                + 0.08 * (4.0 * std::f64::consts::PI * t).cos();
            let weight = sinc * window;
            sum += self.history[index as usize] as f64 * weight;
            weights += weight;
        }
        if weights == 0.0 {
            0.0
        } else {
            (sum / weights) as f32
        }
    }
}

/// One picture from a video.
pub struct Frame {
    /// Seconds into the video where this picture is shown.
    pub time: f64,
    pub image: RgbImage,
}

/// Pictures at (about) `times` seconds, scaled so their long side is at most `max_pixel`, in order (a preview's one
/// frame; reading a whole video uses `frames_each`, which never holds more than one).
pub fn frames(path: &Path, times: &[f64], max_pixel: usize, tolerance: f64) -> Result<Vec<Frame>, String> {
    let mut frames = Vec::new();
    frames_each(path, times, max_pixel, tolerance, |frame| {
        frames.push(frame);
        true
    })?;
    Ok(frames)
}

/// Pictures at (about) `times` seconds, scaled so their long side is at most `max_pixel`, handed to `on_frame` one
/// at a time as they're decoded, in order; `on_frame` returning false stops. Each is the keyframe at or before its
/// time, or (when that's more than `tolerance` seconds back) the frame decoded forward to the time. Times that land
/// on a frame already taken are skipped. Memory stays at one frame for any length: collecting a 2-hour film's 1,200
/// frames first took 1.6 GB.
pub fn frames_each(
    path: &Path,
    times: &[f64],
    max_pixel: usize,
    tolerance: f64,
    mut on_frame: impl FnMut(Frame) -> bool,
) -> Result<(), String> {
    let reader = reader(path, true).map_err(|e| unreadable(&e))?;
    if !has_stream(&reader, FIRST_VIDEO) {
        return Ok(());
    }
    unsafe {
        reader
            .SetStreamSelection(ALL_STREAMS, false)
            .map_err(|e| unreadable(&e))?;
        reader
            .SetStreamSelection(FIRST_VIDEO, true)
            .map_err(|e| unreadable(&e))?;
        let native = reader
            .GetNativeMediaType(FIRST_VIDEO, 0)
            .map_err(|e| unreadable(&e))?;
        let (width, height) = frame_size(&native).ok_or("no video size")?;
        let (w, h) = trovara_core::image::fit(width, height, max_pixel);
        // RGB32 at the size wanted (the reader scales); where it won't scale, at the stored size, scaled here.
        let set = |size: Option<(usize, usize)>| -> windows::core::Result<()> {
            let media_type = MFCreateMediaType()?;
            media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            media_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            if let Some((w, h)) = size {
                media_type.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
            }
            reader.SetCurrentMediaType(FIRST_VIDEO, None, &media_type)
        };
        if set(Some((w & !1, h & !1))).is_err() {
            set(None).map_err(|e| unreadable(&e))?;
        }
        let current = reader
            .GetCurrentMediaType(FIRST_VIDEO)
            .map_err(|e| unreadable(&e))?;
        let (out_w, out_h) = frame_size(&current).ok_or("no video size")?;

        let mut last_time = f64::NEG_INFINITY;
        for &target in times {
            let position = PROPVARIANT::from((target * UNITS) as i64);
            if reader
                .SetCurrentPosition(&GUID::zeroed(), &position)
                .is_err()
            {
                break;
            }
            let mut taken: Option<(f64, IMFSample)> = None;
            // Read until a frame at or after the target, or the keyframe when it's close enough.
            for _ in 0..600 {
                let mut flags = 0u32;
                let mut timestamp = 0i64;
                let mut sample: Option<IMFSample> = None;
                if reader
                    .ReadSample(
                        FIRST_VIDEO,
                        0,
                        None,
                        Some(&mut flags),
                        Some(&mut timestamp),
                        Some(&mut sample),
                    )
                    .is_err()
                {
                    break;
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                let Some(sample) = sample else { continue };
                let time = timestamp as f64 / UNITS;
                let close = time >= target - tolerance;
                taken = Some((time, sample));
                if close {
                    break;
                }
            }
            let Some((time, sample)) = taken else { break };
            if time <= last_time + 1e-3 {
                continue; // the same keyframe again
            }
            last_time = time;
            let mut image = rgb32_image(&sample, out_w, out_h).map_err(|e| unreadable(&e))?;
            if image.width.max(image.height) > max_pixel {
                image = crate::ocr::shrink_image(&image, max_pixel);
            }
            if !on_frame(Frame { time, image }) {
                break;
            }
        }
        Ok(())
    }
}

/// An RGB32 sample (BGRX, top row first through IMF2DBuffer's pitch) as RGB.
unsafe fn rgb32_image(
    sample: &IMFSample,
    width: usize,
    height: usize,
) -> windows::core::Result<RgbImage> {
    let buffer = sample.ConvertToContiguousBuffer()?;
    let mut rgb = Vec::with_capacity(width * height * 3);
    if let Ok(buffer2d) = buffer.cast::<IMF2DBuffer>() {
        let mut scanline0 = std::ptr::null_mut();
        let mut pitch = 0i32;
        buffer2d.Lock2D(&mut scanline0, &mut pitch)?;
        for y in 0..height {
            let row = scanline0.offset(y as isize * pitch as isize);
            let row = std::slice::from_raw_parts(row, width * 4);
            for p in row.chunks_exact(4) {
                rgb.extend_from_slice(&[p[2], p[1], p[0]]);
            }
        }
        buffer2d.Unlock2D()?;
    } else {
        let mut data = std::ptr::null_mut();
        let mut length = 0u32;
        buffer.Lock(&mut data, None, Some(&mut length))?;
        let stride = width * 4;
        let bytes = std::slice::from_raw_parts(data, (length as usize).min(stride * height));
        for y in 0..height {
            for p in bytes[y * stride..(y + 1) * stride].chunks_exact(4) {
                rgb.extend_from_slice(&[p[2], p[1], p[0]]);
            }
        }
        buffer.Unlock()?;
    }
    Ok(RgbImage::new(width, height, rgb))
}

/// 8×8 average hash, as the reference's: gray cells brighter than the mean are 1. Two frames within 4 bits of each
/// other show the same shot.
pub fn average_hash(image: &RgbImage) -> u64 {
    let side = 8;
    let mut cells = [0u32; 64];
    for cy in 0..side {
        let y0 = cy * image.height / side;
        let y1 = image
            .height
            .min((y0 + 1).max((cy + 1) * image.height / side));
        for cx in 0..side {
            let x0 = cx * image.width / side;
            let x1 = image.width.min((x0 + 1).max((cx + 1) * image.width / side));
            let (mut sum, mut n) = (0u32, 0u32);
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = (y * image.width + x) * 3;
                    sum += (image.pixels[p] as u32 * 299
                        + image.pixels[p + 1] as u32 * 587
                        + image.pixels[p + 2] as u32 * 114)
                        / 1000;
                    n += 1;
                }
            }
            cells[cy * side + cx] = sum.checked_div(n).unwrap_or(0);
        }
    }
    let mean = cells.iter().sum::<u32>() / 64;
    cells
        .iter()
        .enumerate()
        .filter(|(_, &cell)| cell > mean)
        .fold(0u64, |hash, (i, _)| hash | 1 << i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_keeps_a_tone() {
        // A 440 Hz tone at 44.1 kHz → 16 kHz: same frequency (zero crossings), same level.
        let input: Vec<f32> = (0..44_100)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 44_100.0).sin() * 0.5)
            .collect();
        let mut resampler = Resampler::new(44_100, 16_000);
        let mut out = resampler.process(&input[..20_000]);
        out.extend(resampler.process(&input[20_000..]));
        out.extend(resampler.finish());
        assert!((out.len() as i64 - 16_000).abs() < 80, "{}", out.len());
        let middle = &out[2000..14_000];
        let crossings = middle
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        assert!(
            (crossings as f64 - 440.0 * 12_000.0 / 16_000.0).abs() < 3.0,
            "{crossings}"
        );
        let peak = middle.iter().fold(0f32, |m, &x| m.max(x.abs()));
        assert!((peak - 0.5).abs() < 0.03, "{peak}");
    }

    #[test]
    fn hashes_tell_shots_apart() {
        let half = |left: u8, right: u8| {
            let mut pixels = Vec::new();
            for _ in 0..64 {
                for x in 0..64 {
                    let v = if x < 32 { left } else { right };
                    pixels.extend_from_slice(&[v, v, v]);
                }
            }
            RgbImage::new(64, 64, pixels)
        };
        let a = average_hash(&half(20, 230));
        let b = average_hash(&half(30, 220)); // the same shot, a little darker
        let c = average_hash(&half(230, 20)); // another shot
        assert!((a ^ b).count_ones() <= 4);
        assert!((a ^ c).count_ones() > 4);
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), [0.5, 0.5]);
    }
}
