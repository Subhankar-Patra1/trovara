//! The EmbeddingGemma 2 files Trovara runs: ggml-org's Q8_0 GGUF conversion at one pinned revision. Never the F16
//! files: the model's activations overflow float16. (TRD §5.1.)

use std::path::{Path, PathBuf};

pub const REPOSITORY: &str = "ggml-org/embeddinggemma-2-GGUF";
pub const REVISION: &str = "bfcd298762cc34d0357ece5ebdd31791a3a374d8";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFile {
    pub name: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

impl ModelFile {
    /// Hugging Face's address for this file at the pinned revision (it redirects to a CDN that takes HTTP ranges).
    pub fn url(&self) -> String {
        format!(
            "https://huggingface.co/{REPOSITORY}/resolve/{REVISION}/{}",
            self.name
        )
    }
}

/// The text model (310 MB): all that queries need.
pub const TEXT: ModelFile = ModelFile {
    name: "embeddinggemma-2-Q8_0.gguf",
    bytes: 309_855_456,
    sha256: "2188ac1deca4b77dffefd603c2776a9d76d9d74ec01841392982ebb840b09135",
};

/// The image and audio encoders (555 MB).
pub const MEDIA: ModelFile = ModelFile {
    name: "mmproj-embeddinggemma-2-Q8_0.gguf",
    bytes: 554_821_024,
    sha256: "c4a8a52691ecef40618438928bdf9e68379b854e24166f292592353db0aab64f",
};

/// In download order.
pub const ALL: [ModelFile; 2] = [TEXT, MEDIA];

pub const TOTAL_BYTES: u64 = TEXT.bytes + MEDIA.bytes;

/// The pinned revision's folder in the Hugging Face cache: `%HF_HUB_CACHE%`, `%HF_HOME%\hub`, or
/// `~\.cache\huggingface\hub`.
pub fn hugging_face_snapshot() -> Option<PathBuf> {
    let hub = std::env::var_os("HF_HUB_CACHE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HF_HOME").map(|home| PathBuf::from(home).join("hub")))
        .or_else(|| {
            crate::paths::home().map(|home| home.join(".cache").join("huggingface").join("hub"))
        })?;
    Some(
        hub.join(format!("models--{}", REPOSITORY.replace('/', "--")))
            .join("snapshots")
            .join(REVISION),
    )
}

/// The files that aren't in `folder` yet. Only verified files get their final name, so being there is enough.
pub fn missing(folder: &Path) -> Vec<ModelFile> {
    ALL.into_iter()
        .filter(|file| !folder.join(file.name).is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_pinned() {
        assert_eq!(
            TEXT.url(),
            "https://huggingface.co/ggml-org/embeddinggemma-2-GGUF/resolve/\
             bfcd298762cc34d0357ece5ebdd31791a3a374d8/embeddinggemma-2-Q8_0.gguf"
        );
        assert_eq!(TOTAL_BYTES, 864_676_480);
    }
}
