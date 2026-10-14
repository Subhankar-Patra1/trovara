//! The llama.cpp, mtmd and ggml calls Trovara makes. Only functions whose arguments are pointers, integers and bools
//! are declared here; the ones that pass structs by value go through the C shim (shim/trovara_llama.c), compiled
//! against the real headers. Signatures follow llama.cpp b11461 (scripts/build-llama.ps1 pins it).

#![allow(non_camel_case_types, dead_code)]

use std::ffi::{c_char, c_int, c_void};

pub type llama_token = i32;
pub type llama_pos = i32;
pub type llama_seq_id = i32;

#[repr(C)]
pub struct llama_model {
    _private: [u8; 0],
}
#[repr(C)]
pub struct llama_context {
    _private: [u8; 0],
}
#[repr(C)]
pub struct llama_vocab {
    _private: [u8; 0],
}
#[repr(C)]
pub struct llama_batch_ext {
    _private: [u8; 0],
}
#[repr(C)]
pub struct mtmd_context {
    _private: [u8; 0],
}
#[repr(C)]
pub struct mtmd_bitmap {
    _private: [u8; 0],
}
#[repr(C)]
pub struct mtmd_input_chunks {
    _private: [u8; 0],
}
#[repr(C)]
pub struct mtmd_input_chunk {
    _private: [u8; 0],
}
#[repr(C)]
pub struct ggml_backend_reg {
    _private: [u8; 0],
}
#[repr(C)]
pub struct ggml_backend_device {
    _private: [u8; 0],
}

/// `enum llama_process_type`
pub const LLAMA_PROCESS_TYPE_ENCODE: c_int = 0;
/// `enum mtmd_input_chunk_type`
pub const MTMD_INPUT_CHUNK_TYPE_TEXT: c_int = 0;
/// `enum ggml_log_level`
pub const GGML_LOG_LEVEL_WARN: c_int = 3;
pub const GGML_LOG_LEVEL_CONT: c_int = 5;
/// `enum ggml_backend_dev_type`
pub const GGML_BACKEND_DEVICE_TYPE_CPU: c_int = 0;

pub type ggml_log_callback =
    Option<unsafe extern "C" fn(level: c_int, text: *const c_char, user_data: *mut c_void)>;

extern "C" {
    // shim/trovara_llama.c
    pub fn trv_pick_device(preference: *const c_char) -> *mut ggml_backend_device;
    pub fn trv_model_load(
        path: *const c_char,
        device: *mut ggml_backend_device,
    ) -> *mut llama_model;
    pub fn trv_context_init(
        model: *mut llama_model,
        max_tokens: u32,
        max_sequences: u32,
        threads: i32,
    ) -> *mut llama_context;
    pub fn trv_mtmd_init(
        path: *const c_char,
        model: *const llama_model,
        threads: i32,
        device: *mut ggml_backend_device,
    ) -> *mut mtmd_context;
    pub fn trv_batch_add_embd(
        batch: *mut llama_batch_ext,
        seq: llama_seq_id,
        row: *const f32,
        n_embd: usize,
    ) -> i32;
    pub fn trv_bitmap_from_buf(
        ctx: *mut mtmd_context,
        buf: *const u8,
        len: usize,
        is_video: *mut bool,
    ) -> *mut mtmd_bitmap;
    pub fn trv_tokenize_media(
        ctx: *mut mtmd_context,
        chunks: *mut mtmd_input_chunks,
        bitmap: *const mtmd_bitmap,
    ) -> i32;
}

#[link(name = "llama", kind = "dylib")]
extern "C" {
    pub fn llama_backend_init();
    pub fn llama_log_set(callback: ggml_log_callback, user_data: *mut c_void);
    pub fn llama_model_free(model: *mut llama_model);
    pub fn llama_free(ctx: *mut llama_context);
    pub fn llama_model_get_vocab(model: *const llama_model) -> *const llama_vocab;
    pub fn llama_model_n_embd_inp(model: *const llama_model) -> i32;
    pub fn llama_model_n_embd_out(model: *const llama_model) -> i32;
    pub fn llama_vocab_eos(vocab: *const llama_vocab) -> llama_token;
    pub fn llama_tokenize(
        vocab: *const llama_vocab,
        text: *const c_char,
        text_len: i32,
        tokens: *mut llama_token,
        n_tokens_max: i32,
        add_special: bool,
        parse_special: bool,
    ) -> i32;
    pub fn llama_batch_ext_init(ctx: *mut llama_context) -> *mut llama_batch_ext;
    pub fn llama_batch_ext_free(batch: *mut llama_batch_ext);
    pub fn llama_batch_ext_clear(batch: *mut llama_batch_ext);
    pub fn llama_batch_ext_add_token(
        batch: *mut llama_batch_ext,
        seq_id: llama_seq_id,
        id: llama_token,
    ) -> i32;
    pub fn llama_batch_ext_set_output_embd(
        batch: *mut llama_batch_ext,
        idx: i32,
        value: bool,
    ) -> bool;
    pub fn llama_batch_ext_set_pos(
        batch: *mut llama_batch_ext,
        idx: i32,
        pos: *const llama_pos,
    ) -> bool;
    pub fn llama_process(ctx: *mut llama_context, kind: c_int, batch: *mut llama_batch_ext) -> i32;
    pub fn llama_get_embeddings_seq(ctx: *mut llama_context, seq_id: llama_seq_id) -> *mut f32;
}

#[link(name = "mtmd", kind = "dylib")]
extern "C" {
    pub fn mtmd_free(ctx: *mut mtmd_context);
    pub fn mtmd_helper_log_set(callback: ggml_log_callback, user_data: *mut c_void);
    pub fn mtmd_get_audio_sample_rate(ctx: *const mtmd_context) -> c_int;
    pub fn mtmd_bitmap_init(nx: u32, ny: u32, data: *const u8) -> *mut mtmd_bitmap;
    pub fn mtmd_bitmap_init_from_audio(n_samples: usize, data: *const f32) -> *mut mtmd_bitmap;
    pub fn mtmd_bitmap_get_nx(bitmap: *const mtmd_bitmap) -> u32;
    pub fn mtmd_bitmap_get_ny(bitmap: *const mtmd_bitmap) -> u32;
    pub fn mtmd_bitmap_get_data(bitmap: *const mtmd_bitmap) -> *const u8;
    pub fn mtmd_bitmap_get_n_bytes(bitmap: *const mtmd_bitmap) -> usize;
    pub fn mtmd_bitmap_is_audio(bitmap: *const mtmd_bitmap) -> bool;
    pub fn mtmd_bitmap_free(bitmap: *mut mtmd_bitmap);
    pub fn mtmd_input_chunks_init() -> *mut mtmd_input_chunks;
    pub fn mtmd_input_chunks_size(chunks: *const mtmd_input_chunks) -> usize;
    pub fn mtmd_input_chunks_get(
        chunks: *const mtmd_input_chunks,
        idx: usize,
    ) -> *const mtmd_input_chunk;
    pub fn mtmd_input_chunks_free(chunks: *mut mtmd_input_chunks);
    pub fn mtmd_input_chunk_get_type(chunk: *const mtmd_input_chunk) -> c_int;
    pub fn mtmd_input_chunk_get_tokens_text(
        chunk: *const mtmd_input_chunk,
        n_tokens: *mut usize,
    ) -> *const llama_token;
    pub fn mtmd_input_chunk_get_n_tokens(chunk: *const mtmd_input_chunk) -> usize;
    pub fn mtmd_encode_chunk(ctx: *mut mtmd_context, chunk: *const mtmd_input_chunk) -> i32;
    pub fn mtmd_get_output_embd(ctx: *mut mtmd_context) -> *mut f32;
}

#[link(name = "ggml", kind = "dylib")]
extern "C" {
    pub fn ggml_backend_load_all();
    pub fn ggml_backend_load_all_from_path(dir_path: *const c_char);
    pub fn ggml_backend_reg_count() -> usize;
    pub fn ggml_backend_reg_get(index: usize) -> *mut ggml_backend_reg;
    pub fn ggml_backend_reg_name(reg: *mut ggml_backend_reg) -> *const c_char;
    pub fn ggml_backend_dev_count() -> usize;
    pub fn ggml_backend_dev_get(index: usize) -> *mut ggml_backend_device;
    pub fn ggml_backend_dev_name(device: *mut ggml_backend_device) -> *const c_char;
    pub fn ggml_backend_dev_description(device: *mut ggml_backend_device) -> *const c_char;
    pub fn ggml_backend_dev_type(device: *mut ggml_backend_device) -> c_int;
}
