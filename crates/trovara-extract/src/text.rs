//! Plain text files in whatever encoding they were saved in: UTF-8 (with or without a BOM), UTF-16 with a BOM, and
//! otherwise the old Windows code page (Windows-1252, what Notepad wrote before 2019).

use std::path::Path;

use crate::{ExtractError, Result};

pub fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|error| ExtractError::Unreadable(error.to_string()))?;
    Ok(decode(&bytes))
}

pub fn decode(bytes: &[u8]) -> String {
    if let Some((encoding, bom)) = encoding_rs::Encoding::for_bom(bytes) {
        return encoding
            .decode_without_bom_handling(&bytes[bom..])
            .0
            .into_owned();
    }
    // UTF-16 without a BOM: every other byte zero in the start of mostly-ASCII text.
    if bytes.len() >= 8 && bytes.len() % 2 == 0 {
        let sample = &bytes[..bytes.len().min(512)];
        let zeros_odd = sample
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|&&b| b == 0)
            .count();
        let zeros_even = sample.iter().step_by(2).filter(|&&b| b == 0).count();
        let half = sample.len() / 2;
        if zeros_odd * 10 >= half * 9 && zeros_even == 0 {
            return encoding_rs::UTF_16LE
                .decode_without_bom_handling(bytes)
                .0
                .into_owned();
        }
        if zeros_even * 10 >= half * 9 && zeros_odd == 0 {
            return encoding_rs::UTF_16BE
                .decode_without_bom_handling(bytes)
                .0
                .into_owned();
        }
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => encoding_rs::WINDOWS_1252
            .decode_without_bom_handling(bytes)
            .0
            .into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodings() {
        assert_eq!(decode("héllo".as_bytes()), "héllo");
        assert_eq!(decode(b"\xEF\xBB\xBFhi"), "hi");
        assert_eq!(decode(b"\xFF\xFEh\0i\0"), "hi");
        assert_eq!(decode(b"h\0e\0l\0l\0o\0!\0"), "hello!"); // UTF-16LE without a BOM
        assert_eq!(decode(b"caf\xE9 \x80"), "café €"); // Windows-1252
    }
}
