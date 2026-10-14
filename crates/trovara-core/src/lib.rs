//! Trovara's engine, shared by every binary. Platform-specific code lives in `trovara-win`; decoders in
//! `trovara-extract`; the model runtime in `trovara-llama`.

pub mod code;
pub mod image;
pub mod model;
pub mod model_files;
pub mod paths;
pub mod query;
pub mod rules;
pub mod search_text;
pub mod searcher;
pub mod store;
pub mod vectors;

pub use model::{CodeQuery, FileKind, IndexOptions, Modality, Prompts, SegmentKind};
pub use searcher::{SearchHit, SearchOptions, Searcher};
pub use store::{Candidate, FileRecord, IndexStore};
