//! Trovara's Windows platform layer (TRD §6, §9, §10.3, §12.2). So far: WIC image decoding and Windows.Media.Ocr.
//! To come: Media Foundation audio and video (M5), power and priority, file IDs, cloud placeholders and the USN
//! journal client (M6), and shell actions (M7).

#[cfg(windows)]
pub mod media;
#[cfg(windows)]
pub mod ocr;
#[cfg(windows)]
pub mod system;
#[cfg(windows)]
pub mod wic;
