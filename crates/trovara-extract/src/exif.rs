//! What a picture's EXIF says about it: how the camera was held, and when it was taken. Read from the file's headers
//! (JPEG, TIFF, HEIF/HEIC/AVIF, PNG, WebP), not its image data.

use std::io::BufReader;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Exif {
    /// 1–8; 1 (or None) is upright.
    pub orientation: Option<u16>,
    /// DateTimeOriginal as the camera wrote it ("2025:03:12 14:03:51").
    pub taken: Option<String>,
}

pub fn read(path: &Path) -> Option<Exif> {
    let file = std::fs::File::open(path).ok()?;
    let data = exif::Reader::new()
        .read_from_container(&mut BufReader::new(file))
        .ok()?;
    let orientation = data
        .get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|field| field.value.get_uint(0))
        .map(|value| value as u16);
    let taken = data
        .get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
        .or_else(|| data.get_field(exif::Tag::DateTime, exif::In::PRIMARY))
        .and_then(|field| match &field.value {
            exif::Value::Ascii(values) => values
                .first()
                .map(|bytes| String::from_utf8_lossy(bytes).trim().to_string()),
            _ => None,
        })
        .filter(|text| !text.is_empty() && !text.starts_with("0000"));
    Some(Exif { orientation, taken })
}
