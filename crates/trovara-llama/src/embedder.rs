//! EmbeddingGemma 2 on llama.cpp, in this process: the image and audio encoders (mtmd) feed the text model, and its
//! mean-pooled, L2-normalized output is the vector (TRD §5.2).
//!
//! Inputs are framed as the reference frames them: `<bos> text <eos>`, `<bos> <|image> soft tokens <image|> <eos>`
//! and the same for audio. The model is non-causal with no KV cache, so each input (token ids and encoder rows
//! together) goes through one `llama_process` call, up to 16 inputs per batch. Audio past 30 s is dropped, as in the
//! reference.
//!
//! One thread at a time. Dropping it frees the model (about 1 GB, or 0.3 GB text-only).

use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, Once};
use std::time::Instant;

use trovara_core::model_files;

use crate::{ffi, image_sizing, log, BUDGETS};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedError {
    /// The model files aren't there (yet).
    NotFound(String),
    /// The runtime failed: nothing more can be embedded with this embedder.
    Failed(String),
    /// One input couldn't be embedded; its neighbours go on.
    Input(String),
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbedError::NotFound(message)
            | EmbedError::Failed(message)
            | EmbedError::Input(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for EmbedError {}

/// Where the EmbeddingGemma 2 files are.
#[derive(Clone, Debug)]
pub struct Models {
    pub text: PathBuf,
    /// None while only the text part is there.
    pub media: Option<PathBuf>,
}

impl Models {
    /// The folders to look in, first match wins. `TROVARA_MODELS` is the app's choice and then the only place it
    /// looks (the developer's Hugging Face cache mustn't hide a missing download). Without it: the data root's
    /// Models folder, then the Hugging Face cache.
    pub fn folders() -> Vec<PathBuf> {
        if let Some(chosen) = std::env::var_os("TROVARA_MODELS").filter(|value| !value.is_empty()) {
            return vec![PathBuf::from(chosen)];
        }
        let mut folders = vec![trovara_core::paths::models()];
        folders.extend(model_files::hugging_face_snapshot());
        folders
    }

    pub fn locate() -> Result<Models, EmbedError> {
        let folders = Self::folders();
        for folder in &folders {
            let text = folder.join(model_files::TEXT.name);
            if !text.is_file() {
                continue;
            }
            let media = folder.join(model_files::MEDIA.name);
            return Ok(Models {
                text,
                media: media.is_file().then_some(media),
            });
        }
        Err(EmbedError::NotFound(format!(
            "no {} in {}; get it with: trovara-helper download-model",
            model_files::TEXT.name,
            folders
                .iter()
                .map(|folder| folder.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }

    /// Both files are there, so indexing can run (checked without loading anything).
    pub fn complete() -> bool {
        Self::locate()
            .map(|models| models.media.is_some())
            .unwrap_or(false)
    }
}

/// What every embedder reports; it goes into index fingerprints.
pub fn vector_source() -> String {
    format!(
        "google/embeddinggemma-2@{} llama.cpp-vectors{} text-q8_0+mmproj-q8_0 dim768",
        model_files::REVISION,
        crate::VECTOR_VERSION
    )
}

pub struct Embedder {
    model: *mut ffi::llama_model,
    context: *mut ffi::llama_context,
    media: *mut ffi::mtmd_context,
    batch: *mut ffi::llama_batch_ext,
    vocab: *const ffi::llama_vocab,
    /// 768: the output projection.
    embedding_size: usize,
    /// The width of encoder rows fed to the text model.
    row_size: usize,
    max_tokens: usize,
    /// In `batch`.
    sequences: i32,
    tokens: usize,
    position: ffi::llama_pos,
    pub load_seconds: f64,
    pub text_only: bool,
    /// Where the model runs: a GPU's "name (description)", or "CPU".
    pub device: String,
}

// The pointers are owned by this value and only used through `&mut self`.
unsafe impl Send for Embedder {}

const MAX_SEQUENCES: i32 = 16;

impl Embedder {
    /// `text_only`: just the text model (queries). `log`: where llama.cpp's warnings go (stderr when None). Uses a GPU
    /// when ggml found one.
    pub fn new(
        models: &Models,
        text_only: bool,
        log_file: Option<&Path>,
    ) -> Result<Embedder, EmbedError> {
        Self::with_gpu(models, text_only, log_file, true)
    }

    /// `gpu: false` keeps everything on the CPU: after a GPU backend failed the self-check (`self_check`).
    pub fn with_gpu(
        models: &Models,
        text_only: bool,
        log_file: Option<&Path>,
        gpu: bool,
    ) -> Result<Embedder, EmbedError> {
        let started = Instant::now();
        log::send_to(log_file);
        init_backends();
        let _gpu = gpu_lock();
        let media_path = match (&models.media, text_only) {
            (_, true) => None,
            (Some(path), false) => Some(path),
            (None, false) => {
                return Err(EmbedError::NotFound(format!(
                    "{} (images and audio) isn't downloaded yet",
                    model_files::MEDIA.name
                )))
            }
        };
        let device = if gpu {
            pick_device()
        } else {
            std::ptr::null_mut()
        };
        let device_name = describe(device);
        let text_path = c_path(&models.text)?;
        let model = unsafe { ffi::trv_model_load(text_path.as_ptr(), device) };
        if model.is_null() {
            return Err(EmbedError::Failed(format!(
                "can't load {}",
                models.text.display()
            )));
        }
        // One input must fit one micro-batch: 2048 tokens (the reference's cut), which also holds a 1120-token image
        // or a 750-token 30 s clip. Queries are short, so the query encoder reserves less and cuts at 512.
        let max_tokens = if text_only { 512 } else { 2048 };
        let threads = threads();
        let context = unsafe {
            ffi::trv_context_init(model, max_tokens as u32, MAX_SEQUENCES as u32, threads)
        };
        if context.is_null() {
            unsafe { ffi::llama_model_free(model) };
            return Err(EmbedError::Failed(
                "can't create a llama.cpp context".into(),
            ));
        }
        let mut media = std::ptr::null_mut();
        if let Some(path) = media_path {
            let c = c_path(path)?;
            media = unsafe { ffi::trv_mtmd_init(c.as_ptr(), model, threads, device) };
            if media.is_null() {
                unsafe {
                    ffi::llama_free(context);
                    ffi::llama_model_free(model);
                }
                return Err(EmbedError::Failed(format!("can't load {}", path.display())));
            }
        }
        let mut embedder = unsafe {
            Embedder {
                model,
                context,
                media,
                batch: ffi::llama_batch_ext_init(context),
                vocab: ffi::llama_model_get_vocab(model),
                embedding_size: ffi::llama_model_n_embd_out(model) as usize,
                row_size: ffi::llama_model_n_embd_inp(model) as usize,
                max_tokens,
                sequences: 0,
                tokens: 0,
                position: 0,
                load_seconds: 0.0,
                text_only,
                device: device_name,
            }
        };
        if embedder.batch.is_null() {
            return Err(EmbedError::Failed("can't create a llama.cpp batch".into()));
        }
        // The first calls set up the compute graphs (and GPU pipelines): pay that now.
        for _ in 0..2 {
            embedder.embed_texts_inner(&[trovara_core::Prompts::query("warm up")])?;
        }
        embedder.load_seconds = (started.elapsed().as_secs_f64() * 100.0).round() / 100.0;
        log::line(&format!(
            "ready in {} s ({}, on {}, llama.cpp {}, vectors {}, {} threads)",
            embedder.load_seconds,
            if text_only { "text-only" } else { "full" },
            embedder.device,
            crate::LLAMA_TAG,
            crate::VECTOR_VERSION,
            threads
        ));
        Ok(embedder)
    }

    // MARK: Embedding (one at a time per process: see GPU_LOCK)

    /// One L2-normalized vector per text, in order. Texts come already prompted (`Prompts`).
    pub fn embed_texts<S: AsRef<str>>(&mut self, texts: &[S]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let _gpu = gpu_lock();
        self.embed_texts_inner(texts)
    }

    /// An image file's vector at a vision budget (70, 140, 280, 560 or 1120 soft tokens).
    pub fn embed_image_file(&mut self, path: &Path, budget: usize) -> Result<Vec<f32>, EmbedError> {
        let _gpu = gpu_lock();
        self.embed_image_file_inner(path, budget)
    }

    /// An image already decoded (packed RGB, 8 bits), at a vision budget. Resized here to fill the budget.
    pub fn embed_image_rgb(
        &mut self,
        rgb: &[u8],
        width: usize,
        height: usize,
        budget: usize,
    ) -> Result<Vec<f32>, EmbedError> {
        let _gpu = gpu_lock();
        self.embed_image_rgb_inner(rgb, width, height, budget)
    }

    /// An audio file's vector (its first 30 s), decoded by mtmd at the encoder's sample rate.
    pub fn embed_audio_file(&mut self, path: &Path) -> Result<Vec<f32>, EmbedError> {
        let _gpu = gpu_lock();
        self.embed_audio_file_inner(path)
    }

    /// Mono float samples at `sample_rate()` (16 kHz); past 30 s is dropped.
    pub fn embed_audio_samples(&mut self, samples: &[f32]) -> Result<Vec<f32>, EmbedError> {
        let _gpu = gpu_lock();
        self.embed_audio_samples_inner(samples)
    }

    pub fn dimension(&self) -> usize {
        self.embedding_size
    }

    // MARK: Text

    /// One L2-normalized vector per text, in order. Texts come already prompted (`Prompts`).
    fn embed_texts_inner<S: AsRef<str>>(
        &mut self,
        texts: &[S],
    ) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.reset();
        let mut vectors = Vec::with_capacity(texts.len());
        for text in texts {
            let tokens = self.tokenize(text.as_ref());
            if self.sequences == MAX_SEQUENCES || self.tokens + tokens.len() > self.max_tokens {
                vectors.extend(self.run()?);
            }
            self.begin();
            for token in tokens {
                self.add_token(token)?;
            }
        }
        if self.sequences > 0 {
            vectors.extend(self.run()?);
        }
        Ok(vectors)
    }

    /// `<bos> text <eos>`, cut to `max_tokens` the way transformers cuts (keeping both special tokens).
    fn tokenize(&self, text: &str) -> Vec<ffi::llama_token> {
        let bytes = text.as_bytes();
        let mut tokens = vec![0; bytes.len() + 8];
        let mut count = unsafe {
            ffi::llama_tokenize(
                self.vocab,
                bytes.as_ptr().cast(),
                bytes.len() as i32,
                tokens.as_mut_ptr(),
                tokens.len() as i32,
                true,
                true,
            )
        };
        if count < 0 {
            tokens = vec![0; (-count) as usize];
            count = unsafe {
                ffi::llama_tokenize(
                    self.vocab,
                    bytes.as_ptr().cast(),
                    bytes.len() as i32,
                    tokens.as_mut_ptr(),
                    tokens.len() as i32,
                    true,
                    true,
                )
            };
        }
        tokens.truncate(count.max(0) as usize);
        if tokens.len() > self.max_tokens {
            tokens.truncate(self.max_tokens - 1);
            tokens.push(unsafe { ffi::llama_vocab_eos(self.vocab) });
        }
        tokens
    }

    // MARK: Media

    /// An image file's vector at a vision budget (70, 140, 280, 560 or 1120 soft tokens).
    fn embed_image_file_inner(
        &mut self,
        path: &Path,
        budget: usize,
    ) -> Result<Vec<f32>, EmbedError> {
        if !BUDGETS.contains(&budget) {
            return Err(EmbedError::Input(format!(
                "budget must be one of {BUDGETS:?}"
            )));
        }
        let bytes = read(path)?;
        let bitmap = self.decode(&bytes, true, path)?;
        let bitmap = self.fit_image(bitmap, budget, path)?;
        self.embed_bitmap(bitmap, path)
    }

    /// An image already decoded (packed RGB, 8 bits), at a vision budget. Resized here to fill the budget.
    fn embed_image_rgb_inner(
        &mut self,
        rgb: &[u8],
        width: usize,
        height: usize,
        budget: usize,
    ) -> Result<Vec<f32>, EmbedError> {
        self.require_media()?;
        if rgb.len() != width * height * 3 || !BUDGETS.contains(&budget) {
            return Err(EmbedError::Input("bad image size or budget".into()));
        }
        let (target_height, target_width) = image_sizing::target_size(height, width, budget)
            .ok_or_else(|| EmbedError::Input("image too thin to embed".into()))?;
        let resized;
        let pixels = if (target_height, target_width) == (height, width) {
            rgb
        } else {
            resized = image_sizing::resize_bicubic(rgb, width, height, target_width, target_height);
            &resized[..]
        };
        let bitmap = unsafe {
            ffi::mtmd_bitmap_init(target_width as u32, target_height as u32, pixels.as_ptr())
        };
        if bitmap.is_null() {
            return Err(EmbedError::Input("can't make a bitmap".into()));
        }
        self.embed_bitmap(Bitmap(bitmap), Path::new("<pixels>"))
    }

    /// An audio file's vector (its first 30 s), decoded by mtmd at the encoder's sample rate.
    fn embed_audio_file_inner(&mut self, path: &Path) -> Result<Vec<f32>, EmbedError> {
        let bytes = read(path)?;
        let bitmap = self.decode(&bytes, false, path)?;
        let bitmap = self.cut_audio(bitmap, path)?;
        self.embed_bitmap(bitmap, path)
    }

    /// Mono float samples at `sample_rate()` (16 kHz); past 30 s is dropped.
    fn embed_audio_samples_inner(&mut self, samples: &[f32]) -> Result<Vec<f32>, EmbedError> {
        self.require_media()?;
        let limit = 30 * self.sample_rate();
        let samples = &samples[..samples.len().min(limit)];
        if samples.is_empty() {
            return Err(EmbedError::Input("no audio".into()));
        }
        let bitmap = unsafe { ffi::mtmd_bitmap_init_from_audio(samples.len(), samples.as_ptr()) };
        if bitmap.is_null() {
            return Err(EmbedError::Input("can't make an audio bitmap".into()));
        }
        self.embed_bitmap(Bitmap(bitmap), Path::new("<samples>"))
    }

    pub fn sample_rate(&self) -> usize {
        if self.media.is_null() {
            16_000
        } else {
            unsafe { ffi::mtmd_get_audio_sample_rate(self.media) as usize }
        }
    }

    fn require_media(&self) -> Result<(), EmbedError> {
        if self.media.is_null() {
            Err(EmbedError::Failed(
                "this embedder was started text-only".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn decode(&mut self, bytes: &[u8], image: bool, path: &Path) -> Result<Bitmap, EmbedError> {
        self.require_media()?;
        let mut is_video = false;
        let bitmap = unsafe {
            ffi::trv_bitmap_from_buf(self.media, bytes.as_ptr(), bytes.len(), &mut is_video)
        };
        if is_video {
            return Err(EmbedError::Input(format!(
                "video goes in as frames and sound, not as a file: {}",
                path.display()
            )));
        }
        if bitmap.is_null() {
            return Err(EmbedError::Input(format!("can't read {}", path.display())));
        }
        let bitmap = Bitmap(bitmap);
        if unsafe { ffi::mtmd_bitmap_is_audio(bitmap.0) } == image {
            return Err(EmbedError::Input(format!(
                "not {}: {}",
                if image { "an image" } else { "audio" },
                path.display()
            )));
        }
        Ok(bitmap)
    }

    fn fit_image(&self, bitmap: Bitmap, budget: usize, path: &Path) -> Result<Bitmap, EmbedError> {
        let (width, height) = unsafe {
            (
                ffi::mtmd_bitmap_get_nx(bitmap.0) as usize,
                ffi::mtmd_bitmap_get_ny(bitmap.0) as usize,
            )
        };
        let (target_height, target_width) = image_sizing::target_size(height, width, budget)
            .ok_or_else(|| {
                EmbedError::Input(format!("image too thin to embed: {}", path.display()))
            })?;
        if (target_height, target_width) == (height, width) {
            return Ok(bitmap);
        }
        let pixels = unsafe {
            std::slice::from_raw_parts(ffi::mtmd_bitmap_get_data(bitmap.0), width * height * 3)
        };
        let resized =
            image_sizing::resize_bicubic(pixels, width, height, target_width, target_height);
        let out = unsafe {
            ffi::mtmd_bitmap_init(target_width as u32, target_height as u32, resized.as_ptr())
        };
        if out.is_null() {
            return Err(EmbedError::Input(format!(
                "can't resize {}",
                path.display()
            )));
        }
        Ok(Bitmap(out))
    }

    fn cut_audio(&self, bitmap: Bitmap, path: &Path) -> Result<Bitmap, EmbedError> {
        let samples =
            unsafe { ffi::mtmd_bitmap_get_n_bytes(bitmap.0) } / std::mem::size_of::<f32>();
        let limit = 30 * self.sample_rate();
        if samples <= limit {
            return Ok(bitmap);
        }
        let data = unsafe { ffi::mtmd_bitmap_get_data(bitmap.0) }.cast::<f32>();
        let prefix = unsafe { ffi::mtmd_bitmap_init_from_audio(limit, data) };
        if prefix.is_null() {
            return Err(EmbedError::Input(format!(
                "can't cut {} at 30 s",
                path.display()
            )));
        }
        Ok(Bitmap(prefix))
    }

    fn embed_bitmap(&mut self, bitmap: Bitmap, path: &Path) -> Result<Vec<f32>, EmbedError> {
        let chunks = Chunks(unsafe { ffi::mtmd_input_chunks_init() });
        let status = unsafe { ffi::trv_tokenize_media(self.media, chunks.0, bitmap.0) };
        if status != 0 {
            return Err(EmbedError::Input(format!(
                "can't tokenize {} ({status})",
                path.display()
            )));
        }
        self.reset();
        self.begin();
        let count = unsafe { ffi::mtmd_input_chunks_size(chunks.0) };
        for index in 0..count {
            let chunk = unsafe { ffi::mtmd_input_chunks_get(chunks.0, index) };
            if unsafe { ffi::mtmd_input_chunk_get_type(chunk) } == ffi::MTMD_INPUT_CHUNK_TYPE_TEXT {
                let mut n = 0usize;
                let tokens = unsafe { ffi::mtmd_input_chunk_get_tokens_text(chunk, &mut n) };
                if tokens.is_null() {
                    continue;
                }
                for &token in unsafe { std::slice::from_raw_parts(tokens, n) } {
                    self.add_token(token)?;
                }
            } else {
                if unsafe { ffi::mtmd_encode_chunk(self.media, chunk) } != 0 {
                    return Err(EmbedError::Input(format!(
                        "the encoder failed on {}",
                        path.display()
                    )));
                }
                let rows = unsafe { ffi::mtmd_get_output_embd(self.media) };
                if rows.is_null() {
                    return Err(EmbedError::Input(format!(
                        "the encoder failed on {}",
                        path.display()
                    )));
                }
                let n = unsafe { ffi::mtmd_input_chunk_get_n_tokens(chunk) };
                for row in 0..n {
                    let index = unsafe {
                        ffi::trv_batch_add_embd(
                            self.batch,
                            self.sequences - 1,
                            rows.add(row * self.row_size),
                            self.row_size,
                        )
                    };
                    self.place(index)?;
                }
            }
        }
        let mut vectors = self.run()?;
        vectors
            .pop()
            .ok_or_else(|| EmbedError::Failed("no vector".into()))
    }

    // MARK: Batch

    /// Starts the next input in the batch (each input is its own sequence).
    fn begin(&mut self) {
        self.sequences += 1;
        self.position = 0;
    }

    /// Drops everything added so far, including an input that failed halfway.
    fn reset(&mut self) {
        unsafe { ffi::llama_batch_ext_clear(self.batch) };
        self.sequences = 0;
        self.tokens = 0;
    }

    fn add_token(&mut self, token: ffi::llama_token) -> Result<(), EmbedError> {
        let index =
            unsafe { ffi::llama_batch_ext_add_token(self.batch, self.sequences - 1, token) };
        self.place(index)
    }

    fn place(&mut self, index: i32) -> Result<(), EmbedError> {
        if index < 0 {
            return Err(EmbedError::Input(format!("batch full ({index})")));
        }
        let pos: [ffi::llama_pos; 4] = [self.position, 0, 0, 0];
        let ok = unsafe {
            ffi::llama_batch_ext_set_pos(self.batch, index, pos.as_ptr())
                && ffi::llama_batch_ext_set_output_embd(self.batch, index, true)
        };
        if !ok {
            return Err(EmbedError::Input(format!("can't place token {index}")));
        }
        self.position += 1;
        self.tokens += 1;
        Ok(())
    }

    /// Runs the text model over the batch and returns one L2-normalized vector per input, in order.
    fn run(&mut self) -> Result<Vec<Vec<f32>>, EmbedError> {
        let status =
            unsafe { ffi::llama_process(self.context, ffi::LLAMA_PROCESS_TYPE_ENCODE, self.batch) };
        let result = if status != 0 {
            Err(EmbedError::Failed(format!(
                "llama_process failed ({status})"
            )))
        } else {
            (0..self.sequences)
                .map(|sequence| {
                    let pooled = unsafe { ffi::llama_get_embeddings_seq(self.context, sequence) };
                    if pooled.is_null() {
                        return Err(EmbedError::Failed(format!(
                            "no pooled embedding for input {sequence}"
                        )));
                    }
                    let pooled = unsafe { std::slice::from_raw_parts(pooled, self.embedding_size) };
                    Ok(normalized(pooled))
                })
                .collect()
        };
        self.reset();
        result
    }
}

impl Drop for Embedder {
    fn drop(&mut self) {
        let _gpu = gpu_lock();
        unsafe {
            if !self.batch.is_null() {
                ffi::llama_batch_ext_free(self.batch);
            }
            if !self.media.is_null() {
                ffi::mtmd_free(self.media);
            }
            if !self.context.is_null() {
                ffi::llama_free(self.context);
            }
            if !self.model.is_null() {
                ffi::llama_model_free(self.model);
            }
        }
    }
}

/// L2-normalized, the sum taken in f64 as the reference does.
fn normalized(vector: &[f32]) -> Vec<f32> {
    let sum: f64 = vector.iter().map(|&x| x as f64 * x as f64).sum();
    let scale = if sum > 0.0 {
        (1.0 / sum.sqrt()) as f32
    } else {
        0.0
    };
    vector.iter().map(|&x| x * scale).collect()
}

struct Bitmap(*mut ffi::mtmd_bitmap);

impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe { ffi::mtmd_bitmap_free(self.0) }
    }
}

struct Chunks(*mut ffi::mtmd_input_chunks);

impl Drop for Chunks {
    fn drop(&mut self) {
        unsafe { ffi::mtmd_input_chunks_free(self.0) }
    }
}

fn read(path: &Path) -> Result<Vec<u8>, EmbedError> {
    std::fs::read(path)
        .map_err(|error| EmbedError::Input(format!("can't read {}: {error}", path.display())))
}

fn c_path(path: &Path) -> Result<CString, EmbedError> {
    // llama.cpp takes UTF-8 paths and opens them through the wide-character calls on Windows.
    let text = path
        .to_str()
        .ok_or_else(|| EmbedError::Failed(format!("not a UTF-8 path: {}", path.display())))?;
    CString::new(text).map_err(|_| EmbedError::Failed(format!("bad path: {}", path.display())))
}

/// The threads llama.cpp computes on (what runs on the CPU: all of it on a PC without a GPU): every logical processor
/// up to 8, where llama.cpp's default is 4; `TROVARA_THREADS` says otherwise. Past 8, hybrid CPUs' efficiency cores
/// hold the performance cores back.
pub fn threads() -> i32 {
    if let Some(chosen) = std::env::var("TROVARA_THREADS")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
    {
        return chosen.max(1);
    }
    std::thread::available_parallelism()
        .map(|n| n.get().min(8) as i32)
        .unwrap_or(4)
}

/// llama.cpp's GPU backends aren't safe to load, run and free models on from two threads at once (two embedders in
/// one process on Vulkan crashed in `ggml_backend_sched`). One GPU does one thing at a time anyway, so loading, every
/// embedding call and freeing take this lock. A panic while holding it doesn't poison the next caller.
static GPU_LOCK: Mutex<()> = Mutex::new(());

thread_local! {
    /// This thread holds `GPU_LOCK` (an embedder that fails while loading is freed under the loader's lock).
    static HOLDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The lock, or nothing when this thread already holds it.
struct GpuGuard(Option<MutexGuard<'static, ()>>);

impl Drop for GpuGuard {
    fn drop(&mut self) {
        if self.0.take().is_some() {
            HOLDING.with(|holding| holding.set(false));
        }
    }
}

fn gpu_lock() -> GpuGuard {
    if HOLDING.with(|holding| holding.get()) {
        return GpuGuard(None);
    }
    let guard = GPU_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    HOLDING.with(|holding| holding.set(true));
    GpuGuard(Some(guard))
}

static INIT: Once = Once::new();

/// llama.cpp's process-wide setup, once: the log, then ggml's backends (DLLs loaded at run time: the best CPU variant
/// for this processor, and Vulkan or CUDA when their DLLs are there and a device works). They're looked for in
/// `TROVARA_BACKENDS`, then beside this executable, then wherever ggml itself looks.
fn init_backends() {
    INIT.call_once(|| unsafe {
        ffi::llama_log_set(Some(log::callback), std::ptr::null_mut());
        ffi::mtmd_helper_log_set(Some(log::callback), std::ptr::null_mut());
        let mut folders = Vec::new();
        if let Some(chosen) = std::env::var_os("TROVARA_BACKENDS").filter(|value| !value.is_empty())
        {
            folders.push(PathBuf::from(chosen));
        }
        if let Some(folder) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
        {
            folders.push(folder);
        }
        for folder in folders {
            if ffi::ggml_backend_dev_count() > 0 {
                break;
            }
            if let Some(c) = folder.to_str().and_then(|text| CString::new(text).ok()) {
                ffi::ggml_backend_load_all_from_path(c.as_ptr());
            }
        }
        if ffi::ggml_backend_dev_count() == 0 {
            ffi::ggml_backend_load_all();
        }
        ffi::llama_backend_init();
        log::line(&format!("devices: {}", devices().join("; ")));
    });
}

/// The GPU to run on (null: the CPU): `TROVARA_DEVICE` names one ("nvidia", "Vulkan1") or says "cpu"; otherwise a
/// discrete GPU before an integrated one, then the one with the most memory (shim/trovara_llama.c).
fn pick_device() -> *mut ffi::ggml_backend_device {
    let preference = std::env::var("TROVARA_DEVICE")
        .ok()
        .and_then(|value| CString::new(value).ok());
    unsafe { ffi::trv_pick_device(preference.as_ref().map_or(std::ptr::null(), |c| c.as_ptr())) }
}

fn describe(device: *mut ffi::ggml_backend_device) -> String {
    if device.is_null() {
        return "CPU".into();
    }
    unsafe {
        let name = CStr::from_ptr(ffi::ggml_backend_dev_name(device)).to_string_lossy();
        let description =
            CStr::from_ptr(ffi::ggml_backend_dev_description(device)).to_string_lossy();
        format!("{name} ({description})")
    }
}

/// The compute devices ggml found, as "name (description)".
pub fn devices() -> Vec<String> {
    unsafe {
        (0..ffi::ggml_backend_dev_count())
            .map(|index| {
                let device = ffi::ggml_backend_dev_get(index);
                let name = CStr::from_ptr(ffi::ggml_backend_dev_name(device)).to_string_lossy();
                let description =
                    CStr::from_ptr(ffi::ggml_backend_dev_description(device)).to_string_lossy();
                format!("{name} ({description})")
            })
            .collect()
    }
}
