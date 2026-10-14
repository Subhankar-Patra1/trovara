//! A video's subtitles, line by line with their times (PRD F-RD-13, TRD §6.5c), so a line from a film finds the film
//! at that moment. From a subtitle file beside the video (`.srt`, `.vtt`, `.ass`/`.ssa`, the untagged or English one
//! first), else from a text track inside the file: Matroska (`S_TEXT/UTF8`, `S_TEXT/ASS`, `S_TEXT/SSA`,
//! `S_TEXT/WEBVTT`) or MP4 (`tx3g`, `wvtt`). Reading a track never reads the video: Matroska blocks of other tracks
//! are skipped by their sizes after a look at their track number, and a file without a text track stops at its track
//! list. Picture subtitles (PGS, VobSub) would need OCR: they're left out.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// One subtitle line (or a few shown together), seconds into the video.
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// What was found, and where from ("The.Notebook.en.srt", "inside the file (eng)").
#[derive(Clone, Debug)]
pub struct Subtitles {
    pub cues: Vec<Cue>,
    pub source: String,
}

/// Cues read at most from one video (a malformed file can't run away).
const MAX_CUES: usize = 100_000;

/// The video's subtitles: a file beside it first, then a text track inside it. None when there are none.
pub fn for_video(path: &Path) -> Option<Subtitles> {
    if let Some(found) = beside(path) {
        return Some(found);
    }
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    let inside = match extension.as_str() {
        "mkv" | "mka" | "webm" | "mk3d" => matroska(path),
        "mp4" | "m4v" | "mov" | "3gp" => mp4(path),
        _ => None,
    }?;
    (!inside.cues.is_empty()).then_some(inside)
}

// MARK: Files beside the video

/// "movie.srt", then English ones ("movie.en.srt", "movie.eng.srt", "movie.English.srt"), then any other language.
fn beside(video: &Path) -> Option<Subtitles> {
    let folder = video.parent()?;
    let stem = video.file_stem()?.to_string_lossy().to_lowercase();
    let mut candidates: Vec<(u8, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(folder).ok()?.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let Some(extension) = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase) else { continue };
        if !matches!(extension.as_str(), "srt" | "vtt" | "ass" | "ssa") || !name.starts_with(&stem) {
            continue;
        }
        let middle = name[stem.len()..name.len() - extension.len() - 1].trim_matches(|c| c == '.' || c == '_' || c == '-');
        let rank = match middle {
            "" => 0,
            "en" | "eng" | "english" | "en-us" | "en-gb" | "en_us" => 1,
            other if other.contains("english") || other.starts_with("en.") => 2,
            _ => 3,
        };
        candidates.push((rank, path));
    }
    candidates.sort();
    for (_, path) in candidates {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        if bytes.len() > 32 << 20 {
            continue;
        }
        let text = crate::text::decode(&bytes);
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
        let cues = match extension.as_str() {
            "vtt" => parse_vtt(&text),
            "ass" | "ssa" => parse_ass(&text),
            _ => parse_srt(&text),
        };
        if !cues.is_empty() {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            return Some(Subtitles { cues, source: name });
        }
    }
    None
}

// MARK: Text formats

/// "01:02:03,456", "02:03.456", "1:02:03.45" (ASS centiseconds) → seconds.
fn clock(text: &str) -> Option<f64> {
    let text = text.trim().replace(',', ".");
    let parts: Vec<&str> = text.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [h, m, s] => (h.parse::<f64>().ok()?, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        [m, s] => (0.0, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        _ => return None,
    };
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

/// Subtitle text as words: markup and override tags out (`<i>`, `{\an8}`, `\N`), entities read, spaces tidied.
fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut in_tag, mut in_override) = (false, false);
    let text = text.replace("\\N", "\n").replace("\\n", "\n").replace("\\h", " ");
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            '{' => in_override = true,
            '}' if in_override => in_override = false,
            _ if in_tag || in_override => {}
            _ => out.push(c),
        }
    }
    let out = out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ").replace("&quot;", "\"");
    out.lines().map(|line| line.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// SubRip: numbered blocks of "start --> end" and lines.
pub fn parse_srt(text: &str) -> Vec<Cue> {
    let text = text.trim_start_matches('\u{FEFF}').replace("\r\n", "\n").replace('\r', "\n");
    let mut cues = Vec::new();
    for block in text.split("\n\n") {
        let mut lines = block.lines().skip_while(|line| !line.contains("-->"));
        let Some(times) = lines.next() else { continue };
        let Some((start, end)) = times.split_once("-->") else { continue };
        let end = end.split_whitespace().next().unwrap_or_default();
        let (Some(start), Some(end)) = (clock(start), clock(end)) else { continue };
        let body = clean(&lines.collect::<Vec<_>>().join("\n"));
        if !body.is_empty() && cues.len() < MAX_CUES {
            cues.push(Cue { start, end, text: body });
        }
    }
    cues
}

/// WebVTT: like SubRip, with "." in times, optional hours, cue settings after the end, and NOTE/STYLE blocks.
pub fn parse_vtt(text: &str) -> Vec<Cue> {
    let text = text.trim_start_matches('\u{FEFF}').replace("\r\n", "\n");
    let blocks: Vec<&str> =
        text.split("\n\n").filter(|b| !b.starts_with("WEBVTT") && !b.starts_with("NOTE") && !b.starts_with("STYLE")).collect();
    parse_srt(&blocks.join("\n\n"))
}

/// SubStation Alpha / Advanced SSA: the `[Events]` section's Dialogue lines, in its Format's order.
pub fn parse_ass(text: &str) -> Vec<Cue> {
    let mut format: Vec<String> = Vec::new();
    let mut in_events = false;
    let mut cues = Vec::new();
    for line in text.trim_start_matches('\u{FEFF}').lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_events = line.eq_ignore_ascii_case("[events]");
            continue;
        }
        if !in_events {
            continue;
        }
        if let Some(fields) = line.strip_prefix("Format:") {
            format = fields.split(',').map(|f| f.trim().to_ascii_lowercase()).collect();
        } else if let Some(fields) = line.strip_prefix("Dialogue:") {
            if let Some(cue) = ass_dialogue(fields, &format) {
                if cues.len() < MAX_CUES {
                    cues.push(cue);
                }
            }
        }
    }
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    cues
}

fn ass_dialogue(fields: &str, format: &[String]) -> Option<Cue> {
    let default = ["layer", "start", "end", "style", "name", "marginl", "marginr", "marginv", "effect", "text"];
    let names: Vec<&str> = if format.is_empty() { default.to_vec() } else { format.iter().map(String::as_str).collect() };
    let values: Vec<&str> = fields.splitn(names.len(), ',').collect();
    let field = |name: &str| names.iter().position(|n| *n == name).and_then(|i| values.get(i)).map(|v| v.trim());
    let (start, end) = (clock(field("start")?)?, clock(field("end")?)?);
    let text = clean(field("text")?);
    (!text.is_empty()).then_some(Cue { start, end, text })
}

// MARK: Matroska

const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
const TRACKS: u32 = 0x1654_AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_NUMBER: u32 = 0xD7;
const TRACK_TYPE: u32 = 0x83;
const CODEC_ID: u32 = 0x86;
const LANGUAGE: u32 = 0x22_B59C;
const LANGUAGE_BCP47: u32 = 0x22_B59D;
const FLAG_FORCED: u32 = 0x55AA;
const CLUSTER: u32 = 0x1F43_B675;
const CLUSTER_TIMESTAMP: u32 = 0xE7;
const SIMPLE_BLOCK: u32 = 0xA3;
const BLOCK_GROUP: u32 = 0xA0;
const BLOCK: u32 = 0xA1;
const BLOCK_DURATION: u32 = 0x9B;
const SEEK_HEAD: u32 = 0x114D_9B74;
const SEEK: u32 = 0x4DBB;
const SEEK_ID: u32 = 0x53AB;
const SEEK_POSITION: u32 = 0x53AC;
const CUES: u32 = 0x1C53_BB6B;
const CUE_POINT: u32 = 0xBB;
const CUE_TRACK_POSITIONS: u32 = 0xB7;
const CUE_TRACK: u32 = 0xF7;
const CUE_CLUSTER_POSITION: u32 = 0xF1;
const CUE_RELATIVE_POSITION: u32 = 0xF0;
/// Elements of the Segment itself: one of them ends a Cluster of unknown size.
const SEGMENT_CHILDREN: [u32; 7] = [0x114D_9B74, INFO, TRACKS, CLUSTER, 0x1C53_BB6B, 0x1254_C367, 0x1941_A469];

struct Ebml<R: Read + Seek> {
    reader: BufReader<R>,
    position: u64,
    length: u64,
}

impl<R: Read + Seek> Ebml<R> {
    fn byte(&mut self) -> Option<u8> {
        let mut one = [0u8];
        self.reader.read_exact(&mut one).ok()?;
        self.position += 1;
        Some(one[0])
    }

    /// An element ID (its length marker kept, as the IDs are written).
    fn id(&mut self) -> Option<u32> {
        let first = self.byte()?;
        let length = first.leading_zeros() as usize + 1;
        if length > 4 {
            return None;
        }
        let mut id = first as u32;
        for _ in 1..length {
            id = (id << 8) | self.byte()? as u32;
        }
        Some(id)
    }

    /// A size (its marker removed); None for "unknown" (all ones).
    fn size(&mut self) -> Option<Option<u64>> {
        let first = self.byte()?;
        let length = first.leading_zeros() as usize + 1;
        if length > 8 {
            return None;
        }
        let mut value = (first as u64) & ((1u64 << (8 - length)) - 1);
        let mut all_ones = value == (1u64 << (8 - length)) - 1;
        for _ in 1..length {
            let b = self.byte()?;
            all_ones &= b == 0xFF;
            value = (value << 8) | b as u64;
        }
        Some(if all_ones { None } else { Some(value) })
    }

    fn skip(&mut self, bytes: u64) -> Option<()> {
        if self.position + bytes > self.length {
            return None;
        }
        // Within the buffer when it's small (an audio block): no real seek, and no refill of the buffer.
        self.reader.seek_relative(bytes as i64).ok()?;
        self.position += bytes;
        Some(())
    }

    fn seek_to(&mut self, position: u64) -> Option<()> {
        if position >= self.length {
            return None;
        }
        self.reader.seek(SeekFrom::Start(position)).ok()?;
        self.position = position;
        Some(())
    }

    fn bytes(&mut self, count: u64) -> Option<Vec<u8>> {
        if count > 16 << 20 || self.position + count > self.length {
            return None;
        }
        let mut data = vec![0u8; count as usize];
        self.reader.read_exact(&mut data).ok()?;
        self.position += count;
        Some(data)
    }

    fn uint(&mut self, size: u64) -> Option<u64> {
        Some(self.bytes(size.min(8))?.iter().fold(0u64, |v, b| (v << 8) | *b as u64))
    }
}

/// A block's track number (a vint) and how many bytes it took.
fn vint(data: &[u8]) -> Option<(u64, usize)> {
    let first = *data.first()?;
    let length = first.leading_zeros() as usize + 1;
    if length > 8 || data.len() < length {
        return None;
    }
    let mut value = (first as u64) & ((1u64 << (8 - length)) - 1);
    for b in &data[1..length] {
        value = (value << 8) | *b as u64;
    }
    Some((value, length))
}

struct TextTrack {
    number: u64,
    codec: String,
    language: String,
    forced: bool,
}

/// The text subtitle track inside a Matroska file (the English one first, a forced-only track last).
pub fn matroska(path: &Path) -> Option<Subtitles> {
    let file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    // A walk through the clusters reads straight on: with a 1 MB buffer almost every skip over a video or audio block
    // stays inside it (a disk reads a film end to end in seconds, where 400,000 small seeks took a minute). Jumps
    // through the index have their own reader with a small buffer.
    let mut ebml = Ebml { reader: BufReader::with_capacity(1 << 20, file), position: 0, length };
    if ebml.id()? != EBML {
        return None;
    }
    let header = ebml.size()??;
    ebml.skip(header)?;
    if ebml.id()? != SEGMENT {
        return None;
    }
    let segment_size = ebml.size()?;
    let segment_start = ebml.position;
    let segment_end = segment_size.map_or(length, |size| (segment_start + size).min(length));
    let mut scale = 1_000_000u64; // ns per timestamp unit
    let mut track: Option<TextTrack> = None;
    let mut cues: Vec<Cue> = Vec::new();
    let mut tracks_seen = false;
    let mut index_at: Option<u64> = None; // where the Cues (the index) are, from the SeekHead
    while ebml.position < segment_end {
        let Some(id) = ebml.id() else { break };
        let Some(size) = ebml.size() else { break };
        match id {
            SEEK_HEAD => {
                let end = ebml.position + size?;
                while ebml.position < end {
                    let child = ebml.id()?;
                    let child_size = ebml.size()??;
                    if child != SEEK {
                        ebml.skip(child_size)?;
                        continue;
                    }
                    let seek_end = ebml.position + child_size;
                    let (mut target, mut at) = (0u32, None);
                    while ebml.position < seek_end {
                        let field = ebml.id()?;
                        let field_size = ebml.size()??;
                        match field {
                            SEEK_ID => target = ebml.uint(field_size)? as u32,
                            SEEK_POSITION => at = Some(ebml.uint(field_size)?),
                            _ => ebml.skip(field_size)?,
                        }
                    }
                    if target == CUES {
                        index_at = at.map(|at| segment_start + at);
                    }
                }
            }
            INFO => {
                let end = ebml.position + size?;
                while ebml.position < end {
                    let child = ebml.id()?;
                    let child_size = ebml.size()??;
                    if child == TIMESTAMP_SCALE {
                        scale = ebml.uint(child_size)?.max(1);
                    } else {
                        ebml.skip(child_size)?;
                    }
                }
            }
            TRACKS => {
                let end = ebml.position + size?;
                track = text_track(&mut ebml, end)?;
                tracks_seen = true;
                // No text track: nothing in the file to read (the video itself is never walked).
                track.as_ref()?;
            }
            CLUSTER => {
                // Tracks come first in every muxer's files; a file that puts them last has its clusters walked blind.
                if !tracks_seen {
                    return None;
                }
                let chosen = track.as_ref()?;
                // The index says where the track's blocks are: straight to them (a film's 1,500 lines, not its
                // 400,000 video and audio blocks).
                if cues.is_empty() {
                    if let Some(at) = index_at.take() {
                        let mut jumps = Ebml { reader: BufReader::with_capacity(4 << 10, File::open(path).ok()?), position: 0, length };
                        let places = index_places(&mut jumps, at, chosen.number);
                        if !places.is_empty() {
                            for (cluster, relative) in places {
                                read_indexed(&mut jumps, segment_start + cluster, relative, chosen, scale, &mut cues);
                            }
                            break;
                        }
                        // No places for this track in the index: walk the clusters from here.
                    }
                }
                let end = size.map(|s| ebml.position + s);
                read_cluster(&mut ebml, end, chosen, scale, &mut cues)?;
                if cues.len() >= MAX_CUES {
                    break;
                }
            }
            _ => match size {
                Some(size) => ebml.skip(size)?,
                None => break,
            },
        }
    }
    let chosen = track?;
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    cues.dedup_by(|a, b| a.start == b.start && a.text == b.text);
    // A cue without a duration lasts until the next one (3 s at most).
    for i in 0..cues.len() {
        if cues[i].end <= cues[i].start {
            let next = cues.get(i + 1).map_or(cues[i].start + 3.0, |c| c.start);
            cues[i].end = next.min(cues[i].start + 3.0).max(cues[i].start);
        }
    }
    Some(Subtitles { cues, source: format!("inside the file ({})", chosen.language) })
}

/// The index's places for `track`: (cluster position in the segment, block position in the cluster if given).
fn index_places<R: Read + Seek>(ebml: &mut Ebml<R>, at: u64, track: u64) -> Vec<(u64, Option<u64>)> {
    let mut places = Vec::new();
    let back = ebml.position;
    let mut read = || -> Option<()> {
        ebml.seek_to(at)?;
        if ebml.id()? != CUES {
            return None;
        }
        let end = ebml.position + ebml.size()??;
        while ebml.position < end {
            let id = ebml.id()?;
            let size = ebml.size()??;
            if id != CUE_POINT {
                ebml.skip(size)?;
                continue;
            }
            let point_end = ebml.position + size;
            while ebml.position < point_end {
                let child = ebml.id()?;
                let child_size = ebml.size()??;
                if child != CUE_TRACK_POSITIONS {
                    ebml.skip(child_size)?;
                    continue;
                }
                let positions_end = ebml.position + child_size;
                let (mut number, mut cluster, mut relative) = (0, None, None);
                while ebml.position < positions_end {
                    let field = ebml.id()?;
                    let field_size = ebml.size()??;
                    match field {
                        CUE_TRACK => number = ebml.uint(field_size)?,
                        CUE_CLUSTER_POSITION => cluster = Some(ebml.uint(field_size)?),
                        CUE_RELATIVE_POSITION => relative = Some(ebml.uint(field_size)?),
                        _ => ebml.skip(field_size)?,
                    }
                }
                if number == track {
                    if let Some(cluster) = cluster {
                        places.push((cluster, relative));
                    }
                }
            }
            if places.len() >= MAX_CUES {
                break;
            }
        }
        Some(())
    };
    let _ = read();
    let _ = ebml.seek_to(back);
    places
}

/// The block the index points at: its cluster's timestamp, then the block (or, without a block position, every
/// block of the track in that cluster).
fn read_indexed<R: Read + Seek>(
    ebml: &mut Ebml<R>,
    cluster_at: u64,
    relative: Option<u64>,
    track: &TextTrack,
    scale: u64,
    cues: &mut Vec<Cue>,
) -> Option<()> {
    ebml.seek_to(cluster_at)?;
    if ebml.id()? != CLUSTER {
        return None;
    }
    let size = ebml.size()?;
    let data = ebml.position;
    let Some(relative) = relative else {
        return read_cluster(ebml, size.map(|s| data + s), track, scale, cues);
    };
    // The cluster's timestamp is its first child in every muxer's files.
    let mut base = 0u64;
    for _ in 0..4 {
        let id = ebml.id()?;
        let child_size = ebml.size()??;
        if id == CLUSTER_TIMESTAMP {
            base = ebml.uint(child_size)?;
            break;
        }
        ebml.skip(child_size)?;
    }
    ebml.seek_to(data + relative)?;
    let id = ebml.id()?;
    let element_size = ebml.size()??;
    read_block_element(ebml, id, element_size, base, track, scale, cues)
}

fn text_track<R: Read + Seek>(ebml: &mut Ebml<R>, end: u64) -> Option<Option<TextTrack>> {
    let mut found: Vec<TextTrack> = Vec::new();
    while ebml.position < end {
        let id = ebml.id()?;
        let size = ebml.size()??;
        if id != TRACK_ENTRY {
            ebml.skip(size)?;
            continue;
        }
        let entry_end = ebml.position + size;
        let (mut number, mut kind, mut codec, mut language, mut forced) = (0, 0, String::new(), String::new(), false);
        let mut bcp47 = String::new();
        while ebml.position < entry_end {
            let child = ebml.id()?;
            let child_size = ebml.size()??;
            match child {
                TRACK_NUMBER => number = ebml.uint(child_size)?,
                TRACK_TYPE => kind = ebml.uint(child_size)?,
                CODEC_ID => codec = String::from_utf8_lossy(&ebml.bytes(child_size)?).trim_end_matches('\0').to_string(),
                LANGUAGE => language = String::from_utf8_lossy(&ebml.bytes(child_size)?).trim_end_matches('\0').to_string(),
                LANGUAGE_BCP47 => bcp47 = String::from_utf8_lossy(&ebml.bytes(child_size)?).trim_end_matches('\0').to_string(),
                FLAG_FORCED => forced = ebml.uint(child_size)? != 0,
                _ => ebml.skip(child_size)?,
            }
        }
        let text = matches!(codec.as_str(), "S_TEXT/UTF8" | "S_TEXT/ASS" | "S_TEXT/SSA" | "S_TEXT/WEBVTT" | "S_TEXT/ASCII");
        // Track type 0x11 is subtitles.
        if kind == 0x11 && text {
            if !bcp47.is_empty() {
                language = bcp47;
            } else if language.is_empty() {
                language = "eng".into(); // Matroska's default when a track says none
            }
            found.push(TextTrack { number, codec, language, forced });
        }
    }
    // English first; a forced track (only signs and foreign lines) last.
    found.sort_by_key(|t| {
        let english = matches!(t.language.to_ascii_lowercase().as_str(), "eng" | "en" | "en-us" | "en-gb");
        (t.forced, !english)
    });
    Some(found.into_iter().next())
}

fn read_cluster<R: Read + Seek>(
    ebml: &mut Ebml<R>,
    end: Option<u64>,
    track: &TextTrack,
    scale: u64,
    cues: &mut Vec<Cue>,
) -> Option<()> {
    let mut base = 0u64;
    loop {
        if end.is_some_and(|end| ebml.position >= end) || ebml.position >= ebml.length {
            return Some(());
        }
        let at = ebml.position;
        let Some(id) = ebml.id() else { return Some(()) };
        if end.is_none() && SEGMENT_CHILDREN.contains(&id) {
            // The next Cluster (or Cues…) after one of unknown size: step back so the Segment loop reads it.
            ebml.reader.seek(SeekFrom::Start(at)).ok()?;
            ebml.position = at;
            return Some(());
        }
        let size = ebml.size()??;
        match id {
            CLUSTER_TIMESTAMP => base = ebml.uint(size)?,
            _ => read_block_element(ebml, id, size, base, track, scale, cues)?,
        }
    }
}

/// One element of a cluster: a SimpleBlock or BlockGroup of the track becomes a cue; anything else is skipped.
fn read_block_element<R: Read + Seek>(
    ebml: &mut Ebml<R>,
    id: u32,
    size: u64,
    base: u64,
    track: &TextTrack,
    scale: u64,
    cues: &mut Vec<Cue>,
) -> Option<()> {
    let seconds = |units: f64| units * scale as f64 / 1e9;
    match id {
        SIMPLE_BLOCK => {
            if let Some((relative, data)) = block(ebml, size, track.number)? {
                let start = seconds(base as f64 + relative as f64);
                push_cue(cues, track, start, start, &data);
            }
        }
        BLOCK_GROUP => {
            let group_end = ebml.position + size;
            let (mut found, mut duration) = (None, None);
            while ebml.position < group_end {
                let child = ebml.id()?;
                let child_size = ebml.size()??;
                match child {
                    BLOCK => found = block(ebml, child_size, track.number)?,
                    BLOCK_DURATION => duration = Some(ebml.uint(child_size)?),
                    _ => ebml.skip(child_size)?,
                }
            }
            if let Some((relative, data)) = found {
                let start = seconds(base as f64 + relative as f64);
                let end = duration.map_or(start, |d| start + seconds(d as f64));
                push_cue(cues, track, start, end, &data);
            }
        }
        _ => ebml.skip(size)?,
    }
    Some(())
}

/// A block of the chosen track: its relative timestamp and payload. Other tracks' blocks are skipped after their
/// first bytes, so the video and the sound are never read.
fn block<R: Read + Seek>(ebml: &mut Ebml<R>, size: u64, wanted: u64) -> Option<Option<(i16, Vec<u8>)>> {
    let head_size = size.min(8);
    let head = ebml.bytes(head_size)?;
    let (number, used) = vint(&head)?;
    if number != wanted {
        ebml.skip(size - head_size)?;
        return Some(None);
    }
    let rest = ebml.bytes(size - head_size)?;
    let mut data = head;
    data.extend(rest);
    if data.len() < used + 3 {
        return Some(None);
    }
    let relative = i16::from_be_bytes([data[used], data[used + 1]]);
    Some(Some((relative, data[used + 3..].to_vec())))
}

fn push_cue(cues: &mut Vec<Cue>, track: &TextTrack, start: f64, end: f64, data: &[u8]) {
    let raw = String::from_utf8_lossy(data);
    let text = match track.codec.as_str() {
        // ReadOrder, Layer, Style, Name, MarginL, MarginR, MarginV, Effect, Text.
        "S_TEXT/ASS" | "S_TEXT/SSA" => raw.splitn(9, ',').nth(8).map(clean).unwrap_or_default(),
        _ => clean(&raw),
    };
    if !text.is_empty() && cues.len() < MAX_CUES {
        cues.push(Cue { start, end, text });
    }
}

// MARK: MP4

/// The text subtitle track inside an MP4/MOV (3GPP timed text `tx3g`, or WebVTT `wvtt`).
pub fn mp4(path: &Path) -> Option<Subtitles> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let moov = find_top_box(&mut file, length, b"moov")?;
    let mut best: Option<(bool, Vec<Cue>, String)> = None;
    for trak in boxes(&moov, b"trak") {
        let Some(mdia) = boxes(trak, b"mdia").next() else { continue };
        let Some(hdlr) = boxes(mdia, b"hdlr").next() else { continue };
        if hdlr.len() < 12 || !matches!(&hdlr[8..12], b"sbtl" | b"text" | b"subt") {
            continue;
        }
        let Some(mdhd) = boxes(mdia, b"mdhd").next() else { continue };
        let (timescale, language) = media_header(mdhd)?;
        let Some(stbl) = boxes(mdia, b"minf").next().and_then(|minf| boxes(minf, b"stbl").next()) else { continue };
        let Some(stsd) = boxes(stbl, b"stsd").next() else { continue };
        let codec = stsd.get(12..16).unwrap_or_default();
        if codec != b"tx3g" && codec != b"wvtt" {
            continue;
        }
        let Some(cues) = mp4_cues(&mut file, stbl, timescale, codec == b"wvtt") else { continue };
        let english = matches!(language.as_str(), "eng" | "en");
        if best.as_ref().is_none_or(|(was_english, cues_before, _)| (english && !was_english) || (english == *was_english && cues.len() > cues_before.len())) {
            best = Some((english, cues, language));
        }
    }
    let (_, cues, language) = best?;
    (!cues.is_empty()).then(|| Subtitles { cues, source: format!("inside the file ({language})") })
}

/// A top-level box read whole (the movie box; a few MB even for a long film), by its type.
fn find_top_box(file: &mut File, length: u64, kind: &[u8; 4]) -> Option<Vec<u8>> {
    let mut at = 0u64;
    while at + 8 <= length {
        file.seek(SeekFrom::Start(at)).ok()?;
        let mut head = [0u8; 16];
        file.read_exact(&mut head[..8]).ok()?;
        let mut size = u32::from_be_bytes(head[0..4].try_into().ok()?) as u64;
        let mut header = 8u64;
        if size == 1 {
            file.read_exact(&mut head[8..16]).ok()?;
            size = u64::from_be_bytes(head[8..16].try_into().ok()?);
            header = 16;
        } else if size == 0 {
            size = length - at;
        }
        if size < header {
            return None;
        }
        if &head[4..8] == kind {
            if size - header > 128 << 20 {
                return None;
            }
            let mut body = vec![0u8; (size - header) as usize];
            file.read_exact(&mut body).ok()?;
            return Some(body);
        }
        at += size;
    }
    None
}

/// The child boxes of `parent`'s payload that are of type `kind`, as their payloads.
fn boxes<'a>(parent: &'a [u8], kind: &'a [u8; 4]) -> impl Iterator<Item = &'a [u8]> + 'a {
    // A sample description box ('stsd') has a version, flags and an entry count before its children.
    let mut at = 0usize;
    std::iter::from_fn(move || {
        while at + 8 <= parent.len() {
            let mut size = u32::from_be_bytes(parent[at..at + 4].try_into().ok()?) as usize;
            let mut header = 8;
            if size == 1 && at + 16 <= parent.len() {
                size = u64::from_be_bytes(parent[at + 8..at + 16].try_into().ok()?) as usize;
                header = 16;
            } else if size == 0 {
                size = parent.len() - at;
            }
            if size < header || at + size > parent.len() {
                return None;
            }
            let found = &parent[at + 4..at + 8] == kind;
            let body = &parent[at + header..at + size];
            at += size;
            if found {
                return Some(body);
            }
        }
        None
    })
}

/// The media header's timescale and ISO 639-2 language.
fn media_header(mdhd: &[u8]) -> Option<(u32, String)> {
    let version = *mdhd.first()?;
    let (scale_at, language_at) = if version == 1 { (20, 32) } else { (12, 20) };
    let timescale = u32::from_be_bytes(mdhd.get(scale_at..scale_at + 4)?.try_into().ok()?);
    let packed = u16::from_be_bytes(mdhd.get(language_at..language_at + 2)?.try_into().ok()?);
    let language: String =
        [(packed >> 10) & 31, (packed >> 5) & 31, packed & 31].iter().map(|c| (*c as u8 + 0x60) as char).collect();
    Some((timescale.max(1), language))
}

fn mp4_cues(file: &mut File, stbl: &[u8], timescale: u32, webvtt: bool) -> Option<Vec<Cue>> {
    let full = |b: &[u8]| -> Vec<u32> { b.chunks_exact(4).map(|c| u32::from_be_bytes(c.try_into().unwrap())).collect() };
    // Durations: (count, delta) pairs.
    let stts = full(boxes(stbl, b"stts").next()?.get(8..)?);
    let mut durations: Vec<u32> = Vec::new();
    for pair in stts.chunks_exact(2) {
        durations.extend(std::iter::repeat_n(pair[1], (pair[0] as usize).min(MAX_CUES)));
    }
    let stsz = boxes(stbl, b"stsz").next()?;
    let fixed = u32::from_be_bytes(stsz.get(4..8)?.try_into().ok()?);
    let count = u32::from_be_bytes(stsz.get(8..12)?.try_into().ok()?) as usize;
    let sizes: Vec<u32> = if fixed != 0 { vec![fixed; count.min(MAX_CUES)] } else { full(stsz.get(12..)?) };
    let offsets: Vec<u64> = match boxes(stbl, b"stco").next() {
        Some(stco) => full(stco.get(8..)?).into_iter().map(u64::from).collect(),
        None => boxes(stbl, b"co64")
            .next()?
            .get(8..)?
            .chunks_exact(8)
            .map(|c| u64::from_be_bytes(c.try_into().unwrap()))
            .collect(),
    };
    let stsc = full(boxes(stbl, b"stsc").next()?.get(8..)?);
    // Each sample's file offset: chunks hold runs of samples (stsc's first_chunk, samples_per_chunk).
    let mut places: Vec<u64> = Vec::with_capacity(sizes.len());
    let runs: Vec<(usize, usize)> = stsc.chunks_exact(3).map(|r| (r[0] as usize, r[1] as usize)).collect();
    let mut sample = 0usize;
    for (chunk, &offset) in offsets.iter().enumerate() {
        let per_chunk = runs.iter().rev().find(|(first, _)| *first <= chunk + 1).map_or(1, |(_, n)| *n);
        let mut at = offset;
        for _ in 0..per_chunk {
            let Some(&size) = sizes.get(sample) else { break };
            places.push(at);
            at += size as u64;
            sample += 1;
        }
    }
    let mut cues = Vec::new();
    let mut time = 0u64;
    for (index, (&place, &size)) in places.iter().zip(&sizes).enumerate() {
        let duration = durations.get(index).copied().unwrap_or(0) as u64;
        let (start, end) = (time as f64 / timescale as f64, (time + duration) as f64 / timescale as f64);
        time += duration;
        if !(2..=64 << 10).contains(&size) {
            continue;
        }
        file.seek(SeekFrom::Start(place)).ok()?;
        let mut data = vec![0u8; size as usize];
        file.read_exact(&mut data).ok()?;
        let text = if webvtt {
            // 'vttc' boxes holding 'payl' (the text); 'vtte' is an empty cue.
            boxes(&data, b"vttc").flat_map(|vttc| boxes(vttc, b"payl")).map(|p| String::from_utf8_lossy(p).into_owned()).collect::<Vec<_>>().join("\n")
        } else {
            let length = u16::from_be_bytes([data[0], data[1]]) as usize;
            String::from_utf8_lossy(data.get(2..2 + length).unwrap_or_default()).into_owned()
        };
        let text = clean(&text);
        if !text.is_empty() && cues.len() < MAX_CUES {
            cues.push(Cue { start, end, text });
        }
    }
    Some(cues)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subrip_and_webvtt() {
        let srt = "1\r\n00:01:02,500 --> 00:01:04,000\r\n<i>It's nice to see you again.</i>\r\n\r\n2\r\n00:01:05,000 --> 00:01:07,250\r\n{\\an8}Is it?\r\nIt's been years.\r\n";
        let cues = parse_srt(srt);
        assert_eq!(cues, [
            Cue { start: 62.5, end: 64.0, text: "It's nice to see you again.".into() },
            Cue { start: 65.0, end: 67.25, text: "Is it?\nIt's been years.".into() },
        ]);
        let vtt = "WEBVTT\n\nNOTE made by hand\n\n00:02.000 --> 00:04.000 align:start\nHello &amp; welcome\n\n01:00:00.000 --> 01:00:01.500\nAn hour in\n";
        let cues = parse_vtt(vtt);
        assert_eq!(cues.len(), 2);
        assert_eq!((cues[0].start, cues[0].text.as_str()), (2.0, "Hello & welcome"));
        assert_eq!(cues[1].start, 3600.0);
    }

    #[test]
    fn advanced_substation() {
        let ass = "[Script Info]\nTitle: x\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:10.50,0:00:12.00,Default,,0,0,0,,{\\i1}Wait, for me{\\i0}\\NPlease\n";
        assert_eq!(parse_ass(ass), [Cue { start: 10.5, end: 12.0, text: "Wait, for me\nPlease".into() }]);
    }

    /// An EBML element: its ID bytes, a one-byte-coded (or eight-byte) size, its payload.
    fn element(id: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        if payload.len() < 127 {
            out.push(0x80 | payload.len() as u8);
        } else {
            out.push(0x01);
            out.extend((payload.len() as u64).to_be_bytes()[1..].iter());
        }
        out.extend(payload);
        out
    }

    #[test]
    fn matroska_text_track_skipping_the_video() {
        let track = |number: u8, kind: u8, codec: &str, language: &str| {
            element(&[0xAE], &[element(&[0xD7], &[number]), element(&[0x83], &[kind]), element(&[0x86], codec.as_bytes()),
                               element(&[0x22, 0xB5, 0x9C], language.as_bytes())].concat())
        };
        let simple = |track: u8, relative: i16, data: &[u8]| {
            let mut payload = vec![0x80 | track];
            payload.extend(relative.to_be_bytes());
            payload.push(0x80);
            payload.extend(data);
            element(&[0xA3], &payload)
        };
        // A block group of track 2: a line at +2.5 s lasting 1.5 s (scale 1 ms).
        let line = {
            let mut payload = vec![0x82];
            payload.extend(2500i16.to_be_bytes());
            payload.push(0);
            payload.extend(b"It's nice to see you <i>again</i>.");
            element(&[0xA0], &[element(&[0xA1], &payload), element(&[0x9B], &1500u16.to_be_bytes())].concat())
        };
        let tracks = element(&[0x16, 0x54, 0xAE, 0x6B], &[track(1, 1, "V_MPEG4/ISO/AVC", "und"), track(2, 0x11, "S_TEXT/UTF8", "eng")].concat());
        let video_frame = vec![0u8; 300];
        // A cluster at 60 s: a video frame, the line, another frame (the frames are skipped unread).
        let cluster = element(&[0x1F, 0x43, 0xB6, 0x75], &[
            element(&[0xE7], &60_000u32.to_be_bytes()),
            simple(1, 0, &video_frame),
            line,
            simple(1, 40, &video_frame),
        ].concat());
        let info = element(&[0x15, 0x49, 0xA9, 0x66], &element(&[0x2A, 0xD7, 0xB1], &1_000_000u32.to_be_bytes()));
        let segment = element(&[0x18, 0x53, 0x80, 0x67], &[info, tracks, cluster].concat());
        let file = [element(&[0x1A, 0x45, 0xDF, 0xA3], &element(&[0x42, 0x82], b"matroska")), segment].concat();
        let path = std::env::temp_dir().join(format!("trovara-subs-{}.mkv", std::process::id()));
        std::fs::write(&path, &file).unwrap();
        let found = matroska(&path).unwrap();
        assert_eq!(found.source, "inside the file (eng)");
        assert_eq!(found.cues, [Cue { start: 62.5, end: 64.0, text: "It's nice to see you again.".into() }]);
        // Without a text track, nothing (and the clusters aren't walked).
        let no_text = element(&[0x16, 0x54, 0xAE, 0x6B], &track(1, 1, "V_MPEG4/ISO/AVC", "und"));
        let segment = element(&[0x18, 0x53, 0x80, 0x67], &no_text);
        std::fs::write(&path, [element(&[0x1A, 0x45, 0xDF, 0xA3], &[]), segment].concat()).unwrap();
        assert!(matroska(&path).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn files_beside_the_video() {
        let folder = std::env::temp_dir().join(format!("trovara-subs-beside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("Film.mkv"), b"x").unwrap();
        std::fs::write(folder.join("Film.hi.srt"), "1\n00:00:01,000 --> 00:00:02,000\nनमस्ते\n").unwrap();
        std::fs::write(folder.join("Film.en.srt"), "1\n00:00:01,000 --> 00:00:02,000\nHello\n").unwrap();
        let found = for_video(&folder.join("Film.mkv")).unwrap();
        assert_eq!((found.source.as_str(), found.cues[0].text.as_str()), ("Film.en.srt", "Hello"));
        let _ = std::fs::remove_dir_all(&folder);
    }
}

#[cfg(test)]
mod probe {
    /// The subtitles of the videos in TROVARA_SUBS_PROBE (paths separated by ";"): how many, how fast, a few lines.
    #[test]
    #[ignore]
    fn subtitles_of_real_videos() {
        let paths = std::env::var("TROVARA_SUBS_PROBE").expect("TROVARA_SUBS_PROBE");
        for path in paths.split(';') {
            let started = std::time::Instant::now();
            let found = super::for_video(std::path::Path::new(path));
            let ms = started.elapsed().as_millis();
            match found {
                Some(subs) => {
                    println!("{path}: {} cues from {} in {ms} ms", subs.cues.len(), subs.source);
                    for cue in subs.cues.iter().filter(|c| c.text.to_lowercase().contains("nice to see you")).take(3) {
                        println!("   {:.1}–{:.1} s: {}", cue.start, cue.end, cue.text.replace('\n', " / "));
                    }
                    for cue in subs.cues.iter().skip(100).take(2) {
                        println!("   {:.1} s: {}", cue.start, cue.text.replace('\n', " / "));
                    }
                }
                None => println!("{path}: none ({ms} ms)"),
            }
        }
    }
}
