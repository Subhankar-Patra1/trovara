//! The reference inputs and vectors that `VECTOR_VERSION` stands for (tests/fixtures/reference: made with llama.cpp
//! b11461; the installer ships the folder as `reference\` beside the helper). Two uses:
//! - the reference-vector test: every llama.cpp update must reproduce them, or bump `VECTOR_VERSION`;
//! - the backend self-check (TRD §5.4): a GPU driver that computes something else is caught on this PC, and Trovara
//!   falls back to the CPU instead of filling the index with vectors that don't match queries.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use trovara_core::Prompts;

use crate::{EmbedError, Embedder, AGREEMENT, VECTOR_VERSION};

/// The texts, as the indexer and the search panel prompt them.
pub fn texts() -> Vec<(&'static str, String)> {
    let long = (1..=60)
        .map(|n| {
            format!(
                "Note {n}: the review moved to room {} because the projector in the main hall failed again, \
                 and the team agreed to send the slides the night before.",
                n % 7 + 2
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    vec![
        ("query", Prompts::query("payment declined error")),
        ("query-bn", Prompts::query("ইলিশ মাছ রান্না")),
        ("query-ar", Prompts::query("متى تغلق المكتبة")),
        (
            "document",
            Prompts::document(
                Some("Tokyo itinerary"),
                "Flight to Tokyo Haneda on November 3. Then the train to Kamakura to see the Great Buddha, and back for \
                 dinner in Shibuya.",
            ),
        ),
        // About 1,800 tokens: past the sliding attention window, so the global layers count as well.
        ("document-long", Prompts::document(None, &long)),
    ]
}

/// The images: (name, file, vision budget). A screenshot at 280 tokens, a photo at 140, and the same photo at 70 as a
/// video frame.
pub const IMAGES: [(&str, &str, usize); 3] = [
    ("screen-280", "screen.png", 280),
    ("photo-140", "photo.jpg", 140),
    ("frame-70", "photo.jpg", 70),
];

/// 16 kHz WAV.
pub const SPEECH: (&str, &str) = ("speech", "speech.wav");

/// Where the reference folder is: `TROVARA_REFERENCE`, `reference\` beside the executable, or the repo's fixtures in
/// dev builds.
pub fn folder() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(chosen) = std::env::var_os("TROVARA_REFERENCE").filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(chosen));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        candidates.push(dir.join("reference"));
    }
    candidates
        .push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/reference"));
    candidates
        .into_iter()
        .find(|folder| folder.join("vectors.json").is_file())
}

/// The stored vectors, checked to be for this pipeline's version.
pub fn stored(folder: &Path) -> Result<BTreeMap<String, Vec<f32>>, EmbedError> {
    let text = std::fs::read_to_string(folder.join("vectors.json")).map_err(|error| {
        EmbedError::NotFound(format!(
            "no reference vectors in {}: {error}",
            folder.display()
        ))
    })?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| EmbedError::Failed(error.to_string()))?;
    if value["vector_version"].as_u64() != Some(VECTOR_VERSION as u64) {
        return Err(EmbedError::Failed(format!(
            "the reference is for vectors {}, this build makes vectors {VECTOR_VERSION}",
            value["vector_version"]
        )));
    }
    serde_json::from_value(value["vectors"].clone())
        .map_err(|error| EmbedError::Failed(error.to_string()))
}

/// Every reference input embedded with `embedder` (text-only embedders do the texts only).
pub fn make(
    embedder: &mut Embedder,
    folder: &Path,
) -> Result<BTreeMap<String, Vec<f32>>, EmbedError> {
    let mut made = BTreeMap::new();
    let texts = texts();
    let prompts: Vec<&str> = texts.iter().map(|(_, text)| text.as_str()).collect();
    for ((name, _), vector) in texts.iter().zip(embedder.embed_texts(&prompts)?) {
        made.insert(name.to_string(), vector);
    }
    if !embedder.text_only {
        for (name, file, budget) in IMAGES {
            made.insert(
                name.to_string(),
                embedder.embed_image_file(&folder.join(file), budget)?,
            );
        }
        made.insert(
            SPEECH.0.to_string(),
            embedder.embed_audio_file(&folder.join(SPEECH.1))?,
        );
    }
    Ok(made)
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// What a check found: each input's cosine with its reference, in name order.
#[derive(Debug, Clone)]
pub struct Check {
    pub cosines: Vec<(String, f32)>,
}

impl Check {
    pub fn worst(&self) -> Option<&(String, f32)> {
        self.cosines.iter().min_by(|a, b| a.1.total_cmp(&b.1))
    }

    pub fn passed(&self) -> bool {
        !self.cosines.is_empty() && self.cosines.iter().all(|(_, cosine)| *cosine >= AGREEMENT)
    }
}

/// An embedder that makes the reference's vectors on this PC (TRD §5.4). With a GPU, the reference inputs are
/// checked once per device set and llama.cpp build (the verdict is kept in `cache`); a GPU whose vectors disagree is
/// dropped for the CPU. Returns the embedder and whether it runs on a GPU.
pub fn open_checked(
    models: &crate::Models,
    text_only: bool,
    log: Option<&Path>,
    cache: &Path,
) -> Result<(Embedder, bool), EmbedError> {
    let embedder = Embedder::new(models, text_only, log)?;
    if embedder.device == "CPU" {
        return Ok((embedder, false));
    }
    let gpu = embedder.device.clone();
    let key = format!("{} {} {gpu}", crate::LLAMA_TAG, VECTOR_VERSION);
    if let Ok(text) = std::fs::read_to_string(cache) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            if value["key"] == key.as_str() {
                return if value["gpu_ok"] == true {
                    Ok((embedder, true))
                } else {
                    drop(embedder);
                    Ok((Embedder::with_gpu(models, text_only, log, false)?, false))
                };
            }
        }
    }
    // The check needs the media encoders too.
    let mut full = if text_only {
        Embedder::new(models, false, log)?
    } else {
        embedder
    };
    let Some(folder) = folder() else {
        return Ok((full, true)); // no reference shipped: trust the GPU
    };
    let verdict = check(&mut full, &folder)?;
    let gpu_ok = verdict.passed();
    let worst = verdict.worst().cloned();
    crate::log::line(&format!(
        "GPU self-check on {gpu}: {} (worst {:?})",
        if gpu_ok {
            "passed"
        } else {
            "FAILED, using the CPU"
        },
        worst
    ));
    let _ = std::fs::write(
        cache,
        serde_json::json!({"key": key, "gpu_ok": gpu_ok, "worst": worst.map(|(name, cosine)| format!("{name} {cosine:.5}"))}).to_string(),
    );
    if gpu_ok {
        if text_only {
            drop(full);
            return Ok((Embedder::new(models, true, log)?, true));
        }
        return Ok((full, true));
    }
    drop(full);
    Ok((Embedder::with_gpu(models, text_only, log, false)?, false))
}

/// Embeds the reference inputs and compares them with the stored vectors.
pub fn check(embedder: &mut Embedder, folder: &Path) -> Result<Check, EmbedError> {
    let stored = stored(folder)?;
    let made = make(embedder, folder)?;
    let cosines = made
        .iter()
        .map(|(name, vector)| {
            let expected = stored
                .get(name)
                .ok_or_else(|| EmbedError::Failed(format!("no reference vector for {name}")))?;
            Ok((name.clone(), cosine(vector, expected)))
        })
        .collect::<Result<Vec<_>, EmbedError>>()?;
    Ok(Check { cosines })
}
