//! EmbeddingGemma 2 on llama.cpp (TRD §5): the FFI to llama.h and mtmd.h, and the embedder that turns text, images
//! and audio into 768-dimensional L2-normalized vectors.

mod embedder;
mod ffi;
pub mod image_sizing;
mod log;
pub mod reference;

pub use embedder::{devices, threads, vector_source, EmbedError, Embedder, Models};

/// Which vectors this pipeline makes. It changes only when the vectors do (checked by the reference-vector test), and
/// indexes record it: a new number means every index embeds again.
pub const VECTOR_VERSION: u32 = 1;

/// The llama.cpp release Trovara builds against (scripts/build-llama.ps1).
pub const LLAMA_TAG: &str = "b11461";

/// The vision soft-token budgets the model accepts.
pub const BUDGETS: [usize; 5] = [70, 140, 280, 560, 1120];

/// Each vector's cosine with its reference must stay at least this: runtimes that agree measure ≥ 0.999, and less is
/// a change people would see in their results.
pub const AGREEMENT: f32 = 0.999;
