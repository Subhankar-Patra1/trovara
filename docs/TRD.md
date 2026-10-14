# TRD: Trovara for Windows

**Technical Requirements Document** · v1.0 · 2026-10-13
**Companion to:** [PRD.md](PRD.md). Requirement IDs (F-…, NFR-…) refer to it.
**Reference only (not part of this project):** the MIT-licensed macOS app DigUp 0.6.1, at `E:\WIndows_Search\DigUp`
(outside the Trovara folder). Where this document says "as DigUp" or names a `.swift` file, that file is the
behavioural spec for the Rust code. No reference name is used in Trovara's folders, crates, binaries or UI. Ported
logic keeps the reference's MIT notice in `THIRD_PARTY_NOTICES.md`.

---

## 1. Architecture

```
┌───────────────────────────────── trovara.exe (Tauri 2, Rust) ─────────────────────────────────┐
│ tray icon · global hotkey · search panel · search window · onboarding · settings · updater  │
│ WebView2 UI (HTML/CSS/JS)  ◄── Tauri IPC ──►  Rust shell: process supervisor, settings      │
└──────────────┬───────────────────────────────┬───────────────────────────┬─────────────────┘
               │ JSON lines (stdin/stdout)      │ JSON lines                 │ JSON lines
               ▼                                ▼                            ▼
   trovara-helper search-service       trovara-helper index-service      trovara-helper download-model
   • index open (SQLite, read)        • crawler, watcher, USN         • HTTP range resume, SHA-256
   • text encoder (310 MB, lazy,      • extractors → embedder         • exits when done
     freed after idle)                • writes SQLite (WAL)
   • vectors in RAM (lazy)            • background priority, EcoQoS
   • renders previews                 • watchdog → exits on runaway
               │                                │
               └───────────── %LOCALAPPDATA%\Trovara\ ──────────────┘
                     Models\  index\index.sqlite  index\code.sqlite  logs\  previews\
               ▲
               │ named pipe \\.\pipe\trovara-<SID> (optional, F-INT)
   PowerToys Command Palette extension · Flow Launcher plugin · scripts
```

**Why separate processes (as DigUp):**
- The model (about 1 GB with the media encoders) lives only in the indexing helper, which exits when the queue is empty.
- A crash or runaway file in a decoder (PDF, video) kills a helper, never the tray app. The supervisor restarts it.
- The search side stays small: keyword search with no vectors loaded; the encoder and vectors load on the first
  meaning query and are released after `--encoder-idle` seconds.
- The protocols already exist and are tested (`Sources/digup/SearchService.swift`, `IndexService.swift`). Trovara keeps
  them, so the existing `shell/` UI ports with few changes.

## 2. Tech stack

| Concern | Choice | Notes |
|---|---|---|
| Language | **Rust** (stable, edition 2021), MSVC toolchain `x86_64-pc-windows-msvc` | One workspace |
| Inference | **llama.cpp** (C++) at the commit DigUp pins, with `scripts/llama.cpp-no-logits.patch` | Built as DLLs with `GGML_BACKEND_DL=ON`, `GGML_CPU_ALL_VARIANTS=ON`, `GGML_VULKAN=ON`; CUDA build separate (optional pack) |
| FFI | Hand-declared plain functions + a small C shim compiled by `cc` against `llama.h`, `mtmd.h`, `mtmd-helper.h` (§20.5) | Own crate `trovara-llama`. No third-party wrapper and no libclang, so the pinned commit is exact. |
| UI shell | **Tauri 2** + WebView2 | Reuse `DigUp/shell/src-tauri` and `shell/ui` as the starting point |
| Windows APIs | **`windows` crate** (windows-rs) | WIC, Media Foundation, WinRT OCR, Shell, power, file IDs, USN, clipboard, IFilter |
| Database | **`rusqlite`** with `bundled` (SQLite ≥ 3.54) | Compile flags: `SQLITE_ENABLE_FTS5`, `SQLITE_THREADSAFE=1`, `SQLITE_DQS=0`, `SQLITE_OMIT_LOAD_EXTENSION` (as DigUp `Package.swift`) |
| Vectors | `half` (f16), `std::arch` AVX2/F16C/AVX-512 intrinsics, `rayon` for parallel scan | Runtime CPU feature detection |
| PDF | **PDFium** (bblanchon/pdfium-binaries, `chromium/8086` as DigUp) via `pdfium-render` | DLL beside the helper |
| Images | **WIC** (primary), `image` crate (fallback: JPEG, PNG, GIF, BMP, TIFF, WebP), `resvg` (SVG), `kamadak-exif` or WIC metadata (EXIF) | |
| HEIC/AVIF | WIC with Microsoft HEIF/AV1 extensions; optional `libheif` pack | §6.1 |
| Audio/video | **Media Foundation** `IMFSourceReader`; optional ffmpeg pack | §6.5 |
| OCR | **`Windows.Media.Ocr`** (WinRT) | §6.2 |
| Docs | `zip` + `quick-xml` (docx, odt, xlsx, pptx); own RTF stripper; `cfb` for legacy .doc (fallback: system IFilter via `LoadIFilter`); `html5ever`/`scraper` for HTML | |
| Code walking | `ignore` crate (ripgrep's .gitignore handling) | |
| Watching | `notify` (ReadDirectoryChangesW) + own USN journal reader (`FSCTL_QUERY_USN_JOURNAL`, `FSCTL_READ_USN_JOURNAL`) | §9 |
| Async / IPC | `tokio` (shell), blocking threads + channels in the helper; `serde_json` lines | |
| HTTP | `reqwest` with `rustls` (model download, update check) | |
| Hashing | `sha2` | Model files |
| Logging | `tracing` + `tracing-appender` (rolling files) | |
| Installer | Tauri bundler → **NSIS** (per-user, no admin); MSIX later | §15 |
| Updates | `tauri-plugin-updater` (signed manifest on GitHub Releases) | |
| Launcher integrations | C# (.NET 8) for the Command Palette extension; Flow Launcher plugin (C# or exe with JSON-RPC) | §13 |

## 3. Repository layout

```
trovara/
├─ Cargo.toml                    workspace
├─ crates/
│  ├─ trovara-core/                engine, platform-neutral: model enums, rules, folder selection, crawler,
│  │                             indexer, index store, searcher, search text, estimator, measured costs
│  ├─ trovara-win/                 Windows platform layer: WIC, OCR, Media Foundation, power, file IDs, USN,
│  │                             cloud placeholders, clipboard, shell open, priority/EcoQoS, IFilter
│  ├─ trovara-extract/             PDF (PDFium), docs (docx/odt/rtf/doc/html), code extractor, EXIF
│  ├─ trovara-llama/               C shim + FFI + Embedder (text, image, audio), image sizing
│  ├─ trovara-helper/            the `trovara-helper` binary: estimate, index, search, status, speed, eval,
│  │                             index-service, search-service, download-model
│  └─ trovara-usn/               the small elevated USN journal reader (§9.3)
├─ app/
│  ├─ src-tauri/                 tray, hotkey, windows, supervisor (from DigUp/shell/src-tauri)
│  └─ ui/                        panel, window, setup, settings (from DigUp/shell/ui)
├─ integrations/
│  ├─ cmdpal/                    PowerToys Command Palette extension (C#)
│  └─ flow-launcher/             Flow Launcher plugin
├─ third_party/
│  ├─ llama.cpp/                 cloned at the pinned commit by scripts/build-llama.ps1 (not in git)
│  └─ patches/llama.cpp-no-logits.patch
├─ scripts/                      build-llama.ps1, fetch-vendor.ps1, make-testbed.py, release.ps1
├─ evals/                        synthetic.json, multilingual.json, code.json (from DigUp)
└─ tests/fixtures/               reference vectors + inputs (from DigUp/Tests/LlamaRuntimeTests/Reference)
```

## 4. Data model

### 4.1 Paths on disk
| What | Where |
|---|---|
| App install | `%LOCALAPPDATA%\Programs\Trovara\` (trovara.exe, trovara-helper.exe, llama/ggml DLLs, pdfium.dll) |
| Data root | `%LOCALAPPDATA%\Trovara\` (portable mode: `.\Data\` beside the exe) |
| Model | `…\Models\embeddinggemma-2-Q8_0.gguf`, `…\Models\mmproj-embeddinggemma-2-Q8_0.gguf` |
| Main index | `…\index\index.sqlite` |
| Code index | `…\index\code.sqlite` |
| Previews | `…\index\previews\` (cleared on start) |
| Logs | `…\logs\app.log`, `index.log`, `search.log` (rolling, 5 × 5 MB) |
| Settings | `…\settings.json` |
| Stopped-file note | `…\index\stopped.json` (watchdog) |

### 4.2 SQLite schema (DigUp schema version 5, plus Windows columns)

```sql
PRAGMA journal_mode = WAL;  PRAGMA synchronous = NORMAL;  PRAGMA foreign_keys = ON;

CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- keys: vector_source (model+pipeline fingerprint), options_fingerprint, schema, created_at, …

CREATE TABLE files(
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,      -- as the crawl reports it (UTF-8, backslashes, no \\?\ prefix)
  path_key TEXT NOT NULL,         -- NEW: Unicode case-folded + NFC path, for range scans (NTFS is case-insensitive)
  kind TEXT NOT NULL,             -- screenshot|image|pdf|doc|audio|video|code
  size INTEGER NOT NULL,
  mtime REAL NOT NULL,            -- seconds since 1970 (from FILETIME)
  inode INTEGER NOT NULL,         -- NTFS 64-bit file reference number (ReFS: low 64 bits of FILE_ID_128)
  device INTEGER NOT NULL,        -- volume serial number
  file_id128 BLOB,                -- NEW: full FILE_ID_128 where the volume has 128-bit IDs (ReFS)
  state TEXT NOT NULL DEFAULT 'pending',   -- pending|done|partial|skipped|failed
  detail TEXT,                    -- why skipped/failed, or how far a long file has been read
  info TEXT,                      -- JSON: width, height, taken, pages, duration, read_to, read_end
  indexed_at REAL
);
CREATE INDEX files_inode ON files(device, inode);
CREATE INDEX files_state ON files(state, kind);
CREATE INDEX files_path_key ON files(path_key);

CREATE TABLE segments(
  id INTEGER PRIMARY KEY,
  file INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,       -- name|image|ocr|page|page_image|chunk|audio|frame|lines|cells
  modality TEXT NOT NULL,   -- none|text|image|audio
  loc REAL,                 -- page number, start second, or first line/cell
  loc_end REAL,
  excerpt TEXT,
  vec BLOB                  -- 768 × float16 LE, L2-normalized (1,536 bytes); NULL for keyword-only segments
);
CREATE INDEX segments_file ON segments(file);

CREATE VIRTUAL TABLE fts USING fts5(
  name, body, original UNINDEXED,
  tokenize = "porter unicode61 remove_diacritics 2 categories 'L* N* Co M*'"
);
-- fts.rowid = segments.id. `body` is normalized text (SearchText); `original` holds the text as written when
-- normalizing changed it (Arabic hamza/harakat, Bengali joiners).
PRAGMA user_version = 5;
```

**Folder scope queries** (as DigUp `IndexStore.pathRanges`): files under a folder are
`path_key >= fold(folder) || '\' AND path_key < fold(folder) || '\' || U+10FFFF`. Windows has no /private/var-style
aliases, but junctions and subst drives do exist, so the real path comes from `GetFinalPathNameByHandleW`, and a second
range is added when it is spelled differently.

**Migrations:** keep `user_version`, and port DigUp's migrations only if importing DigUp indexes (not needed on Windows).
Trovara starts at schema 5 + Windows columns = **Trovara schema 1**.

### 4.3 Kinds (as DigUp `Model.swift`)
- `FileKind`: `screenshot, image, pdf, doc, audio, video, code`. First-pass order is screenshot → image → pdf → doc
  → audio → video.
- `SegmentKind`: `name, image, ocr, page, page_image, chunk, audio, frame, lines, cells`
- `Modality`: `none, text, image, audio`. Scores are normalized **per modality** (§8.2).

### 4.4 Index options and fingerprint (as DigUp `IndexOptions`)
| Option | Value |
|---|---|
| `minImageSide` | 256 px |
| `photoBudget` / `screenshotBudget` / `pageBudget` / `frameBudget` | 140 / 280 / 280 / 70 vision soft tokens (allowed: 70, 140, 280, 560, 1120) |
| `bigPDFPages` | 50 (PDFs up to this many pages are read whole in the first pass) |
| `pdfStep` | 30 pages |
| `docStep` | 40 chunks (~64,000 characters) |
| `maxPages` / `maxChunks` | 5,000 / 5,000 |
| `minPageText` | 200 characters (fewer → embed the rendered page image instead) |
| `frameInterval` | 3.0 s (near-duplicates dropped) |
| `audioWindow` / `audioHop` | 30 s / 25 s, 16 kHz mono |
| `codeStretch` | 1,800 characters (whole lines) |
| `maxCodeBytes` / `maxNotebookBytes` | 1 MiB / 20 MiB |
| Fingerprint (main) | `photo140 shot280 page280 frame70/3.0s audio30.0/25.0s` |
| Fingerprint (code) | `code1 lines1800` |

An index never mixes fingerprints: a mismatch re-embeds the affected segments.

## 5. Embedder (`trovara-llama`)

### 5.1 Model files (pinned, as DigUp `ModelFiles.swift`)
| File | Bytes | SHA-256 |
|---|---|---|
| `embeddinggemma-2-Q8_0.gguf` (text) | 309,855,456 | `2188ac1deca4b77dffefd603c2776a9d76d9d74ec01841392982ebb840b09135` |
| `mmproj-embeddinggemma-2-Q8_0.gguf` (image + audio) | 554,821,024 | `c4a8a52691ecef40618438928bdf9e68379b854e24166f292592353db0aab64f` |

Repo `ggml-org/embeddinggemma-2-GGUF` at revision `bfcd298762cc34d0357ece5ebdd31791a3a374d8`.
URL: `https://huggingface.co/<repo>/resolve/<revision>/<name>`. **Never** use the F16 files: the model's activations
overflow float16.

### 5.2 Pipeline (port of `LlamaEmbedder.swift`)
- Load the text model with `n_gpu_layers = 999`. Load mtmd (mmproj) only for indexing (`textOnly = false`).
- Framing, as the reference: `<bos> text <eos>`, `<bos> <|image> soft tokens <image|> <eos>`, the same for audio.
- Non-causal, no KV cache: each input (token ids + encoder rows) goes through one `llama_process` call. Up to 16
  sequences per batch. One input ≤ 2,048 tokens (the reference's cut; this also holds a 1,120-token image).
- Output: mean-pooled, **L2-normalized**, 768 dimensions → stored as f16.
- Audio past 30 s of one input is dropped by the model, so windows are cut at 30 s (§6.5).
- Image sizing per budget: port `ImageSizing.swift` exactly (it decides the soft-token count).
- `vectorVersion = 1`, `runtime = "llama.cpp-vectors1"`, `dtype = "text-q8_0+mmproj-q8_0"`. These go into
  `meta.vector_source`.
- One thread at a time per embedder instance. `close()` frees the model.

### 5.3 Prompts (as DigUp `Prompts`)
| Use | Prompt |
|---|---|
| Query | `task: search result \| query: {text}` |
| Code query | `task: code retrieval \| query: {text}` |
| Document, page, OCR, chunk, code | `title: {title or "none"} \| text: {text}` |
| Image, audio, frame | no prompt |

### 5.4 Backends
- Ship `ggml-base.dll`, `ggml.dll`, `llama.dll`, `mtmd.dll`, `ggml-cpu-*.dll` (all variants, auto-picked) and
  `ggml-vulkan.dll`.
- Startup self-check: embed the 3 reference inputs (text, image, audio). If any cosine to the stored reference is
  < 0.999, mark that backend bad, fall back to the CPU, and log the device and driver.
- The optional CUDA pack adds `ggml-cuda.dll` + cuBLAS runtime DLLs into `%LOCALAPPDATA%\Trovara\backends\cuda\`. It is
  loaded via `ggml_backend_load` and passes the same self-check.
- Device choice in Settings: Auto (best that passes the self-check) / GPU name / CPU. Dev override:
  `TROVARA_DEVICE=cpu|nvidia|Vulkan1…`.
- **One device, never split** (implemented in M1). Most laptops expose two Vulkan GPUs (the CPU's integrated one and a
  discrete one). By default llama.cpp splits the model across all of them, which made the self-check take 191 s
  instead of 4 s on an RTX 3050 laptop. Auto picks a discrete GPU before an integrated one, then the one with the most
  memory, and loads the text model and the mtmd encoders there with `split_mode = NONE`
  (`trv_pick_device` in the shim).
- **First run compiles GPU pipelines** (the driver caches them per executable): the first embed of each input size
  is 2–5× slower, and the first-ever load can take ~10 s. The app warms them during onboarding (after the model
  download, embed one input of each budget), so the first search and indexing don't pay it.
- **Measured (M1, 2026-10-14, i5-13450HX + RTX 3050 6 GB Laptop, Vulkan):** query 5.7 ms; image 73 / 130 / 259 /
  541 ms at 70 / 140 / 280 / 560 tokens; 30 s audio 55 ms; text model load 1.5 s. The CPU alone: 21.6 ms; 1.0 / 1.9 /
  4.3 / 10.4 s; 340 ms. Vulkan self-check cosines 0.99998–1.00000.

### 5.5 Reference-vector parity (NFR-13)
Port `Tests/LlamaRuntimeTests/ReferenceVectorTests` and its `Reference/` inputs (`photo.jpg`, `speech.wav`, texts).
CI runs it on CPU. A self-hosted GPU runner runs it on Vulkan. Any llama.cpp update must reproduce the vectors, or bump
`vectorVersion`, which forces a re-embed.

## 6. Extractors (`trovara-extract`, `trovara-win`)

Every extractor runs in the indexing helper under the watchdog (§10). It returns segments `{kind, modality, loc,
loc_end, excerpt, text | image | audio}`. Name segments (file name + folder names, keyword only) are added for every
file.

### 6.1 Images
1. Decode with **WIC** (`IWICImagingFactory::CreateDecoderFromFilename`). Apply EXIF orientation. Convert to RGB8.
2. If no WIC codec is available: `image` crate (JPEG/PNG/GIF/BMP/TIFF/WebP), `resvg` for SVG, and the libheif pack for
   HEIC/AVIF.
3. Skip if the short side is < 256 px.
4. Resize to the budget's size (port `ImageSizing.swift`), then embed (`photoBudget` 140, `screenshotBudget` 280).
5. `info`: width, height, `taken` (EXIF DateTimeOriginal).
6. Animated GIF/WebP: first frame. RAW: embedded preview via WIC (Raw Image Extension), else skip with reason.
7. HEIC: needs "HEIF Image Extensions" + "HEVC Video Extensions". Detect with `IWICImagingFactory::CreateComponentEnumerator`.

### 6.2 OCR (`Windows.Media.Ocr`)
- `OcrEngine::TryCreateFromUserProfileLanguages()`. Also create engines for other installed OCR languages
  (`OcrEngine::AvailableRecognizerLanguages`) and pick per image by trying the user's languages first.
- Input: `SoftwareBitmap` (BGRA8). Scale so the image fits `OcrEngine::MaxImageDimension`.
- Used for: screenshots (`ocr` segment: text + vector), and PDF pages with < 200 characters of text layer
  (keywords for scans).
- Mostly-number text → keyword only (`modality = none`), as DigUp.
- Runs on a dedicated STA/MTA thread pool. Each call is about 50–300 ms on a 1080p screenshot. Calibrated by `trovara speed`.

**As built in M4 (2026-10-14), where it differs from the plan above:**
- OCR text is **keyword-only** (no vector) for screenshots and scans alike, as in the reference: a vector of a
  screenshot's words made screenshots outrank better matches in its evals. Meaning comes from the picture's vector.
- Engines: the user-profile engine reads screenshots and scans. An **Arabic** engine (when its OCR pack is installed)
  re-reads PDF pages whose Arabic text layer lost letters (`PageTextProblem::LostLetters`, rendered at 2000 px). The
  reading replaces the layer only when it has ≥ 200 characters, no problem of its own, and Arabic letters. Bengali
  in glyph order isn't OCR'd: Windows' OCR has no Bengali.
- Screenshots are OCR'd at full resolution (up to 4096 px on the long side, then `MaxImageDimension`). Scanned pages
  use the 1400 px render, and the OCR text replaces the layer's when it's longer. Engines are made once per process
  (agile), in the MTA. `RecognizeAsync(...).join()`.
- Measured: a 900×520 screenshot in **30 ms** (i5-13450HX). Windows' OCR reads `_` as a space. That's harmless:
  FTS5 splits `ERR_CARD_DECLINED_4471` into the same words.
- **Reading again:** an index made before OCR worked re-reads its scans and its screenshots without OCR text
  **once** (meta `scans_ocr`). Pictures that failed for a missing codec (the note names a Store extension) are
  retried on every run, so they're read as soon as the extension is installed.
- Dev PC: English OCR only (`en-GB`, `en-US`). To add Arabic: Settings → Time & language → Language & region → add
  Arabic (or `Add-WindowsCapability -Online -Name "Language.OCR~~~ar-SA~0.0.1.0"` as admin).

### 6.3 PDFs (PDFium)
- `FPDF_LoadDocument` (UTF-8 path), page count → `info.pages`.
- Per page: `FPDFText_GetText`. If ≥ 200 characters → `page` segment (text vector). Else render at the `pageBudget`
  size → `page_image` segment (image vector) + OCR text into FTS.
- Pages > `bigPDFPages` (50): first 30 pages in the first pass (`state = partial`, `info.read_to`), the rest in
  `finishLongFiles` steps of 30. Cap at 5,000 pages.
- Encrypted PDFs: try an empty password; else skip "password protected".
- Highlight boxes for the preview: `FPDFText_FindStart` on the page for matched words → rectangles in the `render`
  response.

### 6.3b As built in M3 (2026-10-14)
- **Pictures:** WIC decodes and scales in one step (high-quality cubic, to a long side of 1024 for photos and 1536
  for screenshots, as the reference prepares them). Alpha is composited on white. An icon gives its largest frame.
  SVG is drawn with resvg. The `image` crate fallback wasn't needed: WIC already reads every format it would.
- **Orientation:** EXIF orientation (kamadak-exif) is applied in Rust, except for HEIC/HEIF/AVIF, whose decoders
  apply the container's own rotation (`irot`/`imir`), as the HEIF spec says readers must; EXIF orientation in a HEIC
  is ignored. **Verified 2026-10-14** with the HEVC extension installed: HEICs stored sideways with `irot` decode upright
  (cosine 0.9989 to the upright JPEG, against 0.9407 for an unrotated sideways control). Without the HEVC extension,
  HEIC fails with a message naming the free Store extensions (WIC `MF_E_TRANSFORM_NOT_FOUND` → `NoCodec`).
- **Too small:** long side < 256 px → `skipped` (the reference's rule: its *long* side, not the short one).
- **PDF:** own FFI to `pdfium.dll` (about 20 functions, under one lock), read through a Rust file callback so any
  path opens. Pages ≥ 200 characters of text → `page` (text vector, title = PDF Title or file stem, first 4,000
  characters). Otherwise the page is rendered at 1400 px → `page_image` (image vector, 280 tokens) plus whatever text
  it has, for keywords. Bengali/Hindi text in glyph order → rendered, with the garbled text as a meaning-only vector.
  Lost Arabic letters wait for OCR (M4).
- **Legacy Word (.doc) is read** (the reference couldn't do this outside macOS): CFB container → FIB → piece table
  (Clx/PlcPcd) → UTF-16 or compressed Windows-1252 pieces. Field codes are dropped and field results kept. Encrypted
  files → `password-protected`.
- **docx** reads body + footnotes + endnotes + headers/footers. **odt** reads `content.xml`. RTF uses the reference's
  stripper, with `\'hh` runs decoded together. Text files: BOM, then BOM-less UTF-16 detection, then UTF-8, then
  Windows-1252.
- **Not added to the index until M5:** audio and video (so nothing waits on them).
- Release binaries (M3): `trovara-helper.exe` 8.7 MB. DLLs: llama.cpp ~10 MB, ggml CPU variants ~9 MB,
  `ggml-vulkan.dll` 43 MB (mostly shaders, which compress well), `pdfium.dll` 7 MB.

### 6.4 Documents
| Type | Reader |
|---|---|
| txt, text, md, markdown | UTF-8 / UTF-16 BOM detection; legacy code page fallback (`MultiByteToWideChar` with the system ACP) |
| rtf | Own control-word stripper (`\par`, `\'hh`, `\uN`, skip `\*` destinations) |
| docx | `zip` → `word/document.xml` (+ headers, footers, footnotes) via `quick-xml` |
| odt | `zip` → `content.xml` |
| doc (Word 97–2003) | `cfb` → `WordDocument` + piece table. Fallback: system IFilter (`LoadIFilter`) when Office's filter is installed. |
| html, htm | `scraper`: strip script/style, keep block breaks |
| xlsx, pptx (P1) | `zip` → sharedStrings / sheet XML; slide XML |

Chunking: port DigUp's chunker (passages of about 1,600 characters on sentence or paragraph boundaries; first 40 chunks
in the first pass, the rest later; cap 5,000). Mostly-number text → keyword only.

### 6.5 Audio and video (Media Foundation)
- `MFStartup`. `MFCreateSourceReaderFromURL` with `MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING = TRUE`
  (enables colour conversion and resizing) and `MF_LOW_LATENCY`.
- **Audio:** set the output type to PCM float, 16,000 Hz, mono; the source reader resamples. Stream in 1-second blocks.
  Windows of 30 s, hop 25 s. Each window → `audio` segment, `loc` = start second, `loc_end` = end. Silence-only windows
  (RMS below threshold) are skipped. `info.duration` comes from `MF_PD_DURATION`.
- **Video:** output RGB32. `SetCurrentPosition` every 3 s → `ReadSample` (seek to the nearest keyframe, then decode
  forward). Near-duplicate filter: 64-bit dHash of a 9×8 grey thumbnail; drop a frame whose Hamming distance to the last
  kept frame is ≤ 10 (tune on evals to match DigUp's ~one per shot). Each kept frame → `frame` segment at `frameBudget`
  70, `loc` = second.
- **Soundtrack:** the same audio path on the video's audio stream.
- MPEG-TS (`.ts`, `.mts`, `.m2ts`): check sync bytes (`0x47` every 188 or 192 bytes, as `Rules.isMPEGTransportStream`)
  so TypeScript files aren't read as video.
- Fallback: if MF can't open the file and the ffmpeg pack is installed, use `ffmpeg -f f32le -ac 1 -ar 16000` and
  `ffmpeg -ss T -frames:v 1` (as `Media+Portable.swift`).

### 6.5b As built in M5 (2026-10-14)
- **Audio:** an `IMFSourceReader` with only the audio stream selected. It asks for 16 kHz mono float. If the reader
  refuses that conversion, it takes float at the native rate and resamples in Rust (64-tap windowed sinc,
  Blackman; tested on a 44.1 kHz tone). Blocks are cut into 30 s windows every 25 s **and embedded as they're
  decoded**, so memory stays at one window for any length. A window with RMS ≤ 0.003 (about −50 dBFS) is skipped as
  silence. A tail of ≥ 2 s past the last window's overlap gets its own window. Files with no sound windows →
  `skipped` ("silent or shorter than 2 s"). All as in the reference, minus its temporary WAV files.
- **Video:** advanced video processing gives RGB32, scaled to a long side of 1024 inside the reader where it can be,
  else in Rust. Frames are sampled at `0.5 s + i × max(3 s, duration / 1200)` **by seeking**. A seek lands on the
  keyframe before the time, and that frame is taken when it's within one step; otherwise decoding goes forward to the
  time. So a long video decodes about one frame per sample, not all of them. **One frame per shot:** a frame within
  4 bits (8×8 average hash) of the last kept one is dropped. This follows the reference, replacing the dHash ≤ 10
  guess in §20.4. Each kept frame covers the time until the next. The soundtrack goes through the audio path.
  Segments: `frame` (image vector, 70 tokens, `loc`/`loc_end` in seconds) and `audio`.
- **Measured:** an 18 s 720p clip with 3 shots gave exactly 3 frames (0:00, 0:06, 0:11.9) and 1 sound window. A
  3-minute podcast gave 5 windows, its silent gaps skipped. The whole test folder (podcast, memo, clip, silence)
  indexed in 6.2 s on the RTX 3050.
- **Small indexes:** with a dozen vectors, even a perfect match barely reaches z = 2 (the largest z possible grows
  with the number of vectors). The sea frame scored z 1.97, ranked first but just under the gate. The gate stays at
  the reference's 2.0. `trovara-helper search --z-gate N --word-weight W` exposes both for evals.

### 6.6 Code (port of `CodeExtractor.swift`)
- Walk with the `ignore` crate (respects `.gitignore`, `.ignore`, global gitignore). Skip `codeSkippedFolderNames`,
  submodules and worktrees (`.git` is a file), minified files, licenses, data extensions, secrets, files > 1 MiB.
- Stretches of whole lines up to 1,800 characters (a longer line is a stretch of its own, cap 4,000). `lines` segment:
  `loc`/`loc_end` = first and last line.
- Notebooks: cells without outputs; `cells` segment, `loc` from 1. Skip notebooks > 20 MiB.
- Title = path inside the repo. Prompt `title: {path} | text: {stretch}`.
- Definitions table for name lookups: port the `definers` / `users` keyword sets and the language map, so that
  `code: refreshAccessToken` focuses on the defining line.

### 6.6b As built in M8 (2026-10-14)
- **Where things live.**
  - `trovara-core/src/code.rs` is ported from the reference with its tests:
    - stretches of whole lines (1,800 characters, cut where the code breaks, 2-line overlap inside a block);
    - notebook cells without outputs;
    - the left-out checks: private key, "DO NOT EDIT"/"@generated", minified;
    - the one-line summary;
    - the repo and title (the path from the repo's parent, with `/`);
    - the language;
    - name lookup: `identifier`, `phrase`, `definition`, `first_line`.
  - `rules.rs` has `SOURCE_EXTENSIONS` (+ psm1, bat, cmd, xaml, razor, cshtml, vb, fsx), `CODE_TEXT_EXTENSIONS`,
    `CODE_FILE_NAMES`, `CODE_SKIPPED_FOLDER_NAMES`, `classify_code` and `code_skip_reason` (vendored or built; a
    submodule or worktree, where `.git` is a file).
- **Walk.** The code walk (`crawler::crawl_code`) uses the `ignore` crate with `require_git(true)`: .gitignore,
  `.git\info\exclude` and the global excludes apply inside repos only, as with the reference's `git ls-files`, and
  git needn't be installed. `parents(true)`: a repo's subfolder chosen on its own still follows the repo's rules.
  The one count lost is "ignored by git": the walker doesn't report what it leaves out. Code indexes always sync
  whole, with no change-journal catch-up; that takes 22–39 ms for the test bed.
- **Index.** Code goes into `code.sqlite` beside the main index (`index --code`, `index-service --code`).
  - Segments are `lines` (or `cells`), with `loc`/`loc_end`, the summary as excerpt, and info
    `lines`/`cells`/`repo`/`language`.
  - Embedding uses the text encoder only, with `title: <path in repo> | text: <stretch>`.
  - Fingerprint `code1 lines1800`.
  - The watcher waits for 60 s of quiet, and ignores changes in build, vendor and target folders, so a build
    running all the while can't hold the sync off (F-COD-08).
- **Search.** In the code index, a name (snake_case, camelCase, dotted, `::`, digits) is matched as one FTS phrase
  and counts as a lookup (meaning ×0.5, words ×2). Word weight is 1.0. For the first 10 results, the stretch that
  defines the name is preferred, and `focus` is the defining line. Otherwise `focus` is the first line with the
  query's words, a definition first.
- **search-service.** It opens `code.sqlite` once it exists. `code: true` searches it with
  `task: code retrieval | query: …`. Code results are filtered to `--code-root`/`--code-exclude` (and the `folders`
  op's `code_roots`/`code_skipped`). `reload` takes `code`. `code_off` deletes the code index; the app stops the
  code helper first.
- **eval.** `trovara-helper eval <file> [-n] [--index] [--z-gate] [--word-weight]` is ported; `TROVARA_TESTBED`
  stands in for `~/DigUpTestbed`. The reference's `scripts/testbed/make_code.py` regenerates `evals/code.json`
  identically (41 queries).
- **Measured on the dev PC.**
  - 39 files read in 2.3 s (36 indexed, 3 left out: generated, minified, private key). Vendor, build, dist,
    node_modules, data and keys never reach the index.
  - `eval evals/code.json` gives R@1 = R@5 = MRR = 1.00 on all 41 queries (code 33, code-doc 2, code-name 6).
    That equals the reference's best on this eval. No expected answer is folded.
  - `code: refreshAccessToken` opens at line 21, its definition, not the calls on lines 39 and 44.
  - A search takes 1.6–10 ms.
- **App.**
  - Settings → Code: on/off, code folders with "N files · about T · X MB" each (`estimate --code`, F-COD-07),
    folders left out, and "Open code in" (Automatic, or an installed editor).
  - "Turn Off Code Search…" asks for confirmation, then deletes the code index (F-COD-10). The folders are kept.
  - A second `IndexService` runs `index-service --code`, with its own restarts and model restart. Its progress
    shows in the tray ("Reading code · 3 of 40"), then the main status returns.
  - Code results show numbered lines with the line it opens at marked. Enter, or "Open in VS Code", opens there.
- **Editors** (`editors.rs`, F-COD-09):
  - found in `%LOCALAPPDATA%\Programs`, Program Files, `JetBrains\*\bin` and the Toolbox, `vswhere`, and PATH;
  - the person's choice comes first, then one that's running (Toolhelp snapshot), then the first installed one,
    then the file's own app;
  - each opens at the line its own way: `-g path:line`, `path:line`, `--line N`, `-nN`,
    `/Edit … /Command "Edit.GoTo N"`, `+N`, and `wt nvim +N`.

  On the dev PC it found VS Code, Cursor, Android Studio and Visual Studio 18, and picked VS Code (running).

### 6.7 Classification (port of `Rules.swift`, with Windows additions)
| Rule | Values |
|---|---|
| Doc extensions | txt, text, md, markdown, rtf, docx, doc, odt, html, htm (+ xlsx, pptx in P1) |
| Code/data extensions (skipped outside code search) | DigUp's `codeExtensions` list (py … pyc) |
| Source extensions (code search) | DigUp's `sourceExtensions` list + `psm1`, `bat`, `cmd`, `xaml`, `razor`, `cshtml`, `vb`, `fsx` (project files like `.csproj` stay data) |
| Code-by-name files | DigUp's list (Makefile, Dockerfile, Justfile, …) |
| Project markers | DigUp's list + `*.sln`, `*.csproj`, `*.vcxproj`, `*.fsproj`, `global.json`, `Directory.Build.props` |
| Skipped folder names | DigUp's list (`node_modules`, `bower_components`, `venv`, `site-packages`, `__pycache__`, `DerivedData`, `Pods`, `Carthage`) + `.vs`, `packages` (NuGet, inside projects), `bin`/`obj` inside projects |
| System folders | `%WINDIR%`, `%ProgramFiles%`, `%ProgramFiles(x86)%`, `%ProgramData%`, `%LOCALAPPDATA%`, `%APPDATA%`, `$Recycle.Bin`, `System Volume Information`, `$WinREAgent`, `Recovery`, `PerfLogs`, Trovara's own data root |
| Hidden | `FILE_ATTRIBUTE_HIDDEN` or `FILE_ATTRIBUTE_SYSTEM`, or the name starts with `.` |
| Secrets | DigUp's `isSecret` (pem, key, p12, pfx, keystore, jks, ppk, kdbx, gpg, asc, crt, cer, der, p8, mobileprovision; id_rsa…, .env, secret, credential, .netrc, .npmrc, .pypirc, htpasswd) + `*.rdp`, `*.ovpn`, `*.kdb` |
| Cloud placeholders (never open) | Attributes `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS (0x00400000)`, `FILE_ATTRIBUTE_RECALL_ON_OPEN (0x00040000)`, `FILE_ATTRIBUTE_OFFLINE (0x1000)` → skip with "cloud-only", recheck when attributes change |
| Reparse points | Don't follow directory junctions or symlinks out of the chosen roots (avoid loops) |
| Screenshots | Name prefixes: DigUp's (`Screenshot`, `Screen Shot`, `CleanShot`, `SCR-`) + `Screenshot (N)`, `Screenshot_`, `Snip`, ShareX (`*_ShareX`), Greenshot, Lightshot; **folder**: `Pictures\Screenshots`, `Videos\Captures` (Game Bar stills) |
| Datasets | ≥ 300 same-kind media files in one folder (not a chosen root), ≥ 70% machine-named (`looksMachineNamed` regexes, camera prefixes img, dsc, pxl, mvimg, vid, gopr, dji, photo, whatsapp, screenshot excluded) |

## 7. Search text and keyword search (port of `SearchText.swift`)
- Normalize: NFKC, case fold, Arabic (hamza variants → bare alif, strip harakat and tatweel), Bengali/Indic (strip
  ZWJ/ZWNJ). Keep `original` when it changed.
- Tokenizer: `porter unicode61 remove_diacritics 2 categories 'L* N* Co M*'`, so marks stay inside words.
- Stop words and kind words (`video`, `pdf`, `screenshot`, `photo`…): port the `stopwords` and `kindWords` tables.
- Query patterns: each term quoted. The last term gets `*` when `prefix_last` (typing). A term of 3+ letters that isn't
  in the index as typed also gets `*` (so "grok" finds "Grok4").
- Known limit (as DigUp): languages without spaces (Chinese, Japanese, Thai) aren't split into words. P2: ICU word
  break via `Windows.Data.Text.WordsSegmenter` before FTS insert.

## 8. Ranking (port of `Searcher.swift`, exact)

### 8.1 Meaning
1. Query vector q (768, f32) from the text encoder with the query prompt.
2. Score every stored row: cosine = dot(q, v), v f16 → f32 in the kernel.
3. Per-query scale (`Searcher.meaning`): subtract each modality's mean score (`modalityMeans`; a modality with < 20
   vectors uses the overall mean). Then divide by the standard deviation over rows that set the scale, with a floor of
   0.02, giving z per row. Rows from the "rest of long files" pass are scored but don't set the scale.
4. Per file, keep the best z (and its row).

### 8.2 Words
- `SELECT s.file, s.id, snippet(fts,1,'«','»','…',12), fts.original FROM fts JOIN segments s ON s.id = fts.rowid
  WHERE fts MATCH ? ORDER BY bm25(fts, 3.0, 1.0) LIMIT 1000`. Keep the first hit per file, up to 100 files.
- Coverage per file = the share of query patterns found anywhere in the file.
- **Speed work (M2, measured at 100k segments, two-thirds of them containing the query's common words):**
  - Coverage is computed from the candidates' own segments (≤ 100 files), not from every match of a word. Exact words
    use `MATCH ? AND rowid IN (…)`. Prefix words (`"not"*`, while typing) are read once with
    `rowid BETWEEN min AND max` and intersected in Rust: FTS5 rebuilds a prefix's whole match list on every
    rowid lookup, so `IN` cost ~1.3 ms per candidate segment. Same results as the reference, 54–260 ms → < 1 ms per word.
  - **Deliberate difference from the reference:** the *ranked* query (which picks the ≤ 100 candidate files) leaves
    out words found in more than 20% of segments (and at least 5,000) when the query also has rarer words. BM25 gives
    such words almost no weight, yet ranking their tens of thousands of matches cost 145 ms. They still count fully in
    coverage. A query made only of common words is ranked on all of them.
  - Result: meaning + words 352 → 26 ms (NFR-03 ≤ 150 ms), keyword-only while typing 573 → 14 ms (NFR-02 ≤ 30 ms).
    Guarded by `search_speed_at_100k_vectors` (`cargo test --release -p trovara-core -- --ignored`).
- Embedder concurrency: llama.cpp's Vulkan backend crashed when two models loaded on two threads of one process, so
  loading, every embedding call and freeing take one process-wide, re-entrant lock (`GPU_LOCK`). One GPU does one
  thing at a time anyway.

### 8.3 Fusion
- Candidates = files with best z ≥ **zGate 2.0**, ∪ files with coverage ≥ 0.5.
- `lookup` = query looks like an exact lookup (IDs, codes, a code identifier) → weights (meaning 0.5, words 2 × w);
  else (1, w). w = **2.0** main index, **1.0** code index.
- score = meaningWeight · max(z, 0) + wordsWeight · coverage^1.5 + (1 if the file's kind is hinted by the query)
- Sort by score (ties by file id), limit (default 20). Each hit points at the best-meaning segment if z ≥ gate (or there
  is no keyword hit), else at the keyword segment.
- `strong` count for the "weaker matches" fold: port `Searcher.strongCount`.
- This beat reciprocal-rank fusion in DigUp's evals (R@1 0.88 vs 0.73 synthetic). **Don't change the formula without
  running the evals.**

### 8.4 Vector storage and speed
- In memory: one contiguous `Vec<f16>` (rows × 768), loaded lazily (`loadVectors: false` until the first meaning
  query), in blocks of 1,024 rows; released after encoder idle.
- Kernel: AVX2 + F16C (`_mm256_cvtph_ps` + FMA), AVX-512 when present, scalar fallback. Parallel over blocks with
  `rayon` when rows > 50k.
- Budget: 100k rows = 147 MB f16, scan ≈ 10–20 ms on 8 threads.
- Scale-up path (P1, only if libraries > 300k vectors): an int8 copy (768 B/row, per-row scale) for the scan, top 2,000
  re-scored in f16. > 1M: optional HNSW (`usearch`), with z-scale statistics from a 50k-row sample.

## 9. Crawler, watcher and catch-up

### 9.1 Crawl (port of `Crawler.swift`, `FolderSelection.swift`)
- Roots = chosen folders, minus excluded subfolders (folder selection: whole folder, or chosen subfolders only).
- Enumerate with `FindFirstFileExW(FindExInfoBasic, FIND_FIRST_EX_LARGE_FETCH)` on `\\?\` paths (long paths). Get
  attributes, size, mtime and reparse status from the find data; get the file ID lazily via `GetFileInformationByHandleEx(FileIdInfo)`.
- Volume serial (`GetVolumeInformationByHandleW`) → `device`.
- Diff against `files`: new → pending, changed (size or mtime) → pending, same (device, inode) with a new path →
  **rename** (keep segments, update path and name segment), missing → delete.
- Folders on a disconnected volume (`GetDriveTypeW`/serial missing) → out of `scope`, not deleted.

### 9.2 Live watching
- `notify` with `RecommendedWatcher` (ReadDirectoryChangesW, recursive) per root. Debounce 500 ms, coalesce per path.
- Filter events with `isWalked` (hidden, skipped and system folders, Trovara's own data root) so its own writes don't
  trigger syncs.
- Code roots: debounce 60 s after the last change (F-COD-08).
- Watcher overflow (`ERROR_NOTIFY_ENUM_DIR`) → targeted rescan of that root.

### 9.3 USN journal catch-up (F-IDX-05). Decided: v1 asks once.
On NTFS volumes, reading the journal (`FSCTL_READ_USN_JOURNAL` on a `\\.\C:` volume handle) needs admin rights.
**v1 asks for that once**, during setup (F-ONB-04b) or later in Settings → Indexing.

- **Granting (one UAC prompt):** the app runs `trovara-usn.exe --install` elevated (`ShellExecuteExW` verb `runas`).
  It registers a Task Scheduler task `\Trovara\UsnReader-<user SID>` that runs `trovara-usn.exe --serve` with highest
  privileges, on demand only (no trigger), allowed to be started by that user. Nothing else runs elevated.
- **Use:** on start, the index service records `(journal ID, next USN)` per volume (saved when Trovara exits). It starts
  the task (`ITaskService` → `IRegisteredTask::Run`), and `trovara-usn.exe` reads the journal since that USN for the
  volumes of the chosen roots only. It writes changed **file reference numbers** (no names or content) to
  `\\.\pipe\trovara-usn-<SID>` (DACL: that user only), then exits. The unprivileged index service maps the numbers to
  paths through its own `files(device, inode)` table and `OpenFileById`, and stat-checks only those files.
- **Fallback:** declined, a non-admin account, a non-NTFS volume (exFAT, network), a changed journal ID or a wrapped USN
  → a normal stat-diff crawl (§9.1). The result is the same, only slower.
- **Revoking:** turning it off in Settings runs `trovara-usn.exe --uninstall` (elevated) to delete the task. The
  uninstaller does the same.
- `trovara-usn.exe` is tiny (< 1 MB), signed, takes no arguments from the pipe, and validates that roots are local
  NTFS volumes.
- Expected: catch-up for 1M files in < 2 s with the journal, vs about 30–90 s for a stat crawl.

### 9.4 As built in M6 (2026-10-14)
- **Crawler** (`crawler.rs`): the reference's rules plus Windows': hidden, system and reparse-point folders aren't
  entered (no junction loops). Windows' own folders and Trovara's data root are never walked, even inside a chosen
  `C:\`. Cloud placeholders are never opened. Code projects (Cargo.toml, .sln, .csproj, … anywhere above) keep only
  their screenshots. Folders with ≥ 300 machine-named media files are reported as datasets and skipped. Every
  left-out file is counted by reason.
- **File IDs:** read with `GetFileInformationByHandle` for paths the index doesn't know yet (opening every file each
  sync would cost time). Moves are matched by (volume serial, file reference, size, mtime) among the crawled rows,
  or anywhere in the index when the old path is gone from disk. A renamed folder's files keep their vectors.
  Measured: "1 moved, up to date", nothing embedded again.
- **Watcher:** `notify` (ReadDirectoryChangesW) per root, 500 ms quiet period, then a sync. Only the part of a changed
  path *inside* its root is checked against the rules (a root may itself sit under `Temp` or `.cache`). A new file was
  searchable 0.55 s + model start after it landed (5 s cold, ~1 s with the model warm). Syncs are full crawls
  (≈ 2–30 ms for the test folders); targeted syncs from watcher paths are a later optimization.
- **USN fast catch-up:** `trovara-usn.exe` (journal reader, task install/uninstall) plus the helper's `usn.rs`
  client and `trovara-helper usn install|uninstall|status`. The request and answer are JSON files in the index folder.
  The reader accepts drive letters only and reports only folders inside the requested roots. The first sync of a
  start uses `sync_folders` (each changed folder's own files; new or renamed folders whole; files of deleted subfolders
  removed). Checkpoints are kept per volume in meta (`usn:C:`) and move forward at exit and every 30 minutes idle.
  **Verified 2026-10-14** after the person ran `trovara-helper usn install` (one UAC prompt): the task then runs
  elevated with no prompt. A restart after an offline new file, a deleted file and a renamed folder synced 3 changed
  folders in 152 ms (no full crawl): 1 added, 1 removed, and 1 *moved* (the photo in the renamed folder kept its
  vector). The request and answer live in a fixed `%LOCALAPPDATA%\Trovara\usn` folder (the task's command line can't
  name another index).
- **Estimator** (`estimate [--json]`, per folder and subfolder) and **measured costs** (`costs.json`; indexing tallies
  seconds per unit with model start-up left out; `speed` calibrates from the reference photo). On the test folder
  the estimate was 6.56 s against 6.6 s actual.
- **Watchdog:** 1 s checks of private bytes and per-file time; stops with exit code 75 and `stopped-file.json`. The next
  helper marks the file failed and goes on (tested with a 1 s limit).
- **index-service:** the reference's protocol and loop, plus `pause`/`resume`/`full speed on|off`, `--watch` (stays
  running and small, frees the model after `--linger`), and the power rules in the helper (`--on-battery no`, Battery
  Saver, `--hours HH:MM-HH:MM`, fullscreen). Messages are JSON `note` events. A byte-order mark before a command
  (.NET writers add one) is ignored.

## 10. Indexer and scheduling (port of `Indexer.swift`, `IndexService.swift`, `Watchdog.swift`, `IndexHours.swift`)

### 10.1 Pipeline
```
crawl → classify → queue (order: kind rank, then newest first)
      → reader threads (N = cores/2, decode only) → embed queue (batch: text 16, image 4, audio 2)
      → embedder thread (one, GPU) → writer (SQLite transaction every ~1 s or 200 segments)
```
- One process-wide reading lock for decoders that aren't thread-safe (PDFium is not; use one PDFium thread).
- First pass, then `finishLongFiles` (the rest of long PDFs and docs), then idle → exit.
- After each batch commit, emit `file` events. The shell tells search-service to `reload` at most every few seconds.

### 10.2 index-service protocol (stdin commands, stdout JSON events)
| Direction | Message |
|---|---|
| in | `sync`, `sync code`, `stop`, `backfill on`, `backfill off`, `pause`, `resume`, `full speed on/off` (new) |
| out | `{"event":"missing","folders":[…],"unreadable":[…],"code":b}` · `pruned` · `sync` · `file {new,done,total,path,code}` · `rest {done,total,files,path}` · `summary {states,new,rest,code}` · `waiting {why: battery\|hours\|saver\|idle\|fullscreen}` · `idle` · `rescan {files}` · `stopped {path,why}` · `error {message,fatal}` · `exit` |

Launch: `trovara-helper index-service --backfill [--index DIR] [--exclude PATH]… [--exclude-ext EXT]… ROOTS…`
(as the existing `shell/src-tauri/src/main.rs` `index_launch`).

### 10.3 Priority and power
| Mechanism | API |
|---|---|
| Background CPU/I/O/memory priority | `SetPriorityClass(GetCurrentProcess(), PROCESS_MODE_BACKGROUND_BEGIN)` in the helper. The shell launches it with `CREATE_NO_WINDOW` + `BELOW_NORMAL_PRIORITY_CLASS`. |
| EcoQoS (Windows 11) | `SetProcessInformation(ProcessPowerThrottling, {ControlMask = StateMask = PROCESS_POWER_THROTTLING_EXECUTION_SPEED})`. Off when "full speed". |
| GPU priority | Vulkan queue priority low where the driver allows. Pause the embedder while a fullscreen app has focus (P1). |
| AC/battery | `RegisterPowerSettingNotification(GUID_ACDC_POWER_SOURCE)`, `GetSystemPowerStatus` |
| Battery Saver / Energy Saver | `GUID_POWER_SAVING_STATUS`, `GUID_ENERGY_SAVER_STATUS` → `waiting {why:"saver"}` |
| Set hours | Local-time window check every minute. Changes meanwhile are caught by the watcher's dirty set. |
| Idle (P1) | `GetLastInputInfo` ≥ N minutes |
| Fullscreen / games (P1) | `SHQueryUserNotificationState` → `QUNS_BUSY`, `QUNS_RUNNING_D3D_FULL_SCREEN`, `QUNS_PRESENTATION_MODE` |
| Sleep | `PowerRegisterSuspendResumeNotification`: commit and stop on suspend, rescan dirty on resume |

### 10.3b As built in M6 (2026-10-14): priority, measured, and the decision it forced
**Every lowered priority made indexing 3–8× slower, even on an idle PC.** Windows uses the process priority class as
the GPU scheduling priority. llama.cpp's Vulkan backend submits many small dispatches and waits on fences in between,
so a low-priority waiting thread wakes late and the GPU sits idle. Media test folder, i5-13450HX + RTX 3050, idle:

| `--priority` | audio window | 3-min podcast | 18 s video |
|---|---|---|---|
| normal | 0.11 s | 1.23 s | 0.79 s |
| low (thread below normal) | 0.72–0.94 s | 3.9–7.3 s | 8.2–8.9 s |
| thread-background | 3.69 s | 8.44 s | 8.98 s |
| process-low (BELOW_NORMAL class, throttling opted out) | — | — | 14–22 s for the folder |
| background (PROCESS_MODE_BACKGROUND) / eco (+EcoQoS) | — | — | 16–19 s for the folder |

**Decision: `--priority auto` is the default.** Normal priority on mains power, eco (background mode + EcoQoS) on
battery, switched as the power source changes. Embedding is GPU work that barely uses the CPU, so normal priority
doesn't slow the person's programs; the CPU-heavy parts (decoding, OCR, PDF rendering) are short bursts. As the guard
instead of priority, the **backlog waits while a fullscreen app, a game or a presentation has the screen**
(`SHQueryUserNotificationState`: busy, D3D fullscreen, presentation mode; `--pause-fullscreen no` turns it off). New
files are still read then. The model load always steps out of background mode: at "very low" disk priority, reading
865 MB took 45 s instead of 3. "full speed on" means normal priority whatever the power source. The other levels stay
available for Settings. This replaces "background priority" in PRD G5 and F-IDX-11: the intent is kept (never make
the PC feel slow) by different means.

### 10.4 Watchdog
- A thread checks every 1 s: process private bytes (`GetProcessMemoryInfo` → `PrivateUsage`) and the time on the current
  file (`nowReading`).
- Limits: memory min(8 GB, max(4 GB, RAM/2)); time 15 min per file.
- On breach: write `stopped.json {path, why}`, exit with code `StoppedFile.exitCode`. The supervisor restarts the
  helper, which marks the file `failed` with the reason and goes on.
- Plus: a Win32 **Job Object** on each helper with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` (helpers die with the shell,
  F-TRY-05) and `JobMemoryLimit` as a hard backstop.

### 10.5 Estimates and calibration (port of `Estimator.swift`, `MeasuredCosts.swift`)
- `estimate` walks without reading content: counts by kind, PDF page counts (PDFium header only), media durations (MF
  `MF_PD_DURATION`), code characters (2.4 µs per character, 1,600 per stretch constants from DigUp, then calibrated).
- `trovara speed`: times a fixed embed batch (text, image, audio) and an OCR call, then stores per-PC multipliers.
- After indexing, real per-kind costs are averaged into `MeasuredCosts` (F-IDX-02).

## 11. search-service protocol (keep DigUp's exactly)
Launch: `trovara-helper search-service [--index DIR] [--encoder-idle SECONDS]`. One JSON request per line, reply with the
same `id`.

| op | Request | Reply |
|---|---|---|
| `search` | `{q, code, kinds:[…], limit, meaning, prefix_last}` | `{hits:[…], strong, meaning, ms:{embed, search}}` (hits: path, kind, score, z, cosine, keyword_rank, segment, segment_id, loc, loc_end, excerpt, focus, modified, size, info) |
| `embed_ahead` | `{q, code}` | `{ok:true}` (caches the vector) |
| `text` | `{segment, code}` | `{text}` |
| `render` | `{path, kind: pdf\|video\|image, loc, pixels}` | `{image: path}` (JPEG in `previews\`) + `boxes` for PDF word highlights (new) |
| `reload` | `{}` | `{ok, vectors, code_vectors}` |
| `folders` | `{roots, skipped, types, code_roots, code_skipped}` | `{ok}` |
| `prewarm` | `{}` | `{ok}` (start the encoder, load vectors: panel opening) |
| `status` | `{}` | `{encoder: stopped\|starting\|ready\|unavailable, vectors, …}` |
| `similar` (P1, new) | `{path \| segment_id, kinds, limit}` | like `search` (uses the stored vector as the query) |
| `quit` | `{}` | `{ok}`, exit |

Renders run on a separate worker so typing never waits for a thumbnail. For HEIC/TIFF/RAW, WIC → JPEG. For speed, try
the Explorer thumbnail cache first: `IShellItemImageFactory::GetImage(SIIGBF_THUMBNAILONLY)`.

## 12. Desktop shell (`app/src-tauri`, from DigUp's `shell/`)

### 12.1 Windows and behaviour
| Item | Implementation |
|---|---|
| Panel | One `WebviewWindow`, created hidden at startup and reused: frameless, transparent, `always_on_top`, `skip_taskbar`, 780 × 56 px (grows to 480 px with results). Acrylic or Mica via `window-vibrancy` (falls back to a solid colour on Windows 10). |
| Show | Hotkey → position on the monitor under the cursor (`GetCursorPos` + `MonitorFromPoint`), 1/3 from the top, fade in, `SetForegroundWindow` (with the `AllowSetForegroundWindow` dance from the hotkey handler) |
| Hide | Esc, or blur (`WindowEvent::Focused(false)`), with fade out |
| Hotkey | `tauri-plugin-global-shortcut` (RegisterHotKey). On failure, a "Shortcut in use" toast + Settings |
| Tray | `TrayIconBuilder`. Menu items as PRD F-TRY. Status text updated from index events. Icon states: idle, indexing, paused, error. |
| Single instance | `tauri-plugin-single-instance`: a second launch shows the search window |
| Open at login | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (or `tauri-plugin-autostart`); `--background` flag means start in the tray |
| Dialogs | `tauri-plugin-dialog` for folder pickers |
| Theme | `prefers-color-scheme` + `AppsUseLightTheme` registry for the tray icon variant |
| DPI | Per-monitor v2 (Tauri default). All sizes are logical. |

### 12.2 Actions (`trovara-win::shell`)
| Action | API |
|---|---|
| Open with default app | `ShellExecuteExW` (verb `open`) |
| Show in Explorer | `SHOpenFolderAndSelectItems` (better than `explorer /select,`: no new window if one is open) |
| Copy file | `OleSetClipboard` with a `CF_HDROP` + `Preferred DropEffect = COPY` data object |
| Copy path | `CF_UNICODETEXT` |
| Video/audio at time | Built-in `<video>`/`<audio>` with `currentTime`. VLC: `vlc.exe --start-time=S "path"`. mpv: `mpv --start=S`. Players found via App Paths registry. |
| PDF at page | SumatraPDF `-page N -search "words"` (via App Paths); Edge `msedge.exe "file:///…#page=N"`; Acrobat `Acrobat.exe /A "page=N" path`; else default |
| Code at line | See §12.3 |
| Asset protocol | Tauri `asset:` scope limited at run time to the indexed roots and `previews\` |

### 12.3 Code editors (Windows detection: App Paths, `%LOCALAPPDATA%\Programs`, PATH, JetBrains Toolbox scripts)
| Editor | Open at line |
|---|---|
| VS Code / Insiders / VSCodium / Cursor / Windsurf | `code -g "path:line"` (`code-insiders`, `codium`, `cursor`, `windsurf`), or `vscode://file/path:line` URL |
| Zed | `zed "path:line"` |
| Sublime Text | `subl "path:line"` |
| JetBrains (IDEA, PyCharm, WebStorm, CLion, GoLand, RustRover, PhpStorm, Rider, RubyMine, Android Studio) | `<ide>64.exe --line N "path"` (Toolbox shell scripts if present) |
| Notepad++ | `notepad++.exe -nN "path"` |
| Visual Studio | `devenv.exe /Edit "path"` (then `Edit.GoTo N` via `/Command`) |
| Neovim / Vim | `wt.exe nvim +N "path"` |
| Default | `ShellExecuteExW` |

Choice: Settings value, else the first installed one, preferring a running process (as DigUp's `CodeEditor.chosen`).

### 12.4 Model download (port of `ModelDownload.swift`)
- `trovara-helper download-model` streams progress events `{event: progress, file, bytes, total}`.
- HTTP range resume to `name.part`, SHA-256 while writing, rename only after the hash matches.
- Reuse the Hugging Face cache: `%HF_HUB_CACHE%`, `%HF_HOME%\hub`, `%USERPROFILE%\.cache\huggingface\hub` →
  `models--ggml-org--embeddinggemma-2-GGUF\snapshots\<rev>\<name>` when the size matches (then hash it).
- "Use Files…": pick files, verify size + hash, copy into `Models\`.
- Proxy: honour system proxy (WinHTTP settings / `HTTPS_PROXY`).

### 12.5 Settings file (`settings.json`)
```json
{
  "set_up": true,
  "hotkey": "Ctrl+Shift+Space",
  "folders": ["C:\\Users\\me\\Pictures", "C:\\Users\\me\\Documents"],
  "subfolder_choices": {"C:\\Users\\me\\Documents": ["Work", "Lease"]},
  "skipped": ["C:\\Users\\me\\Documents\\Old"],
  "skip_types": ["heic"],
  "index_on_battery": true,
  "index_hours": {"on": false, "start": "22:00", "end": "06:00"},
  "index_when_idle": {"on": false, "minutes": 5},
  "pause_fullscreen": true,
  "background_throttle": true,
  "code": {"on": false, "folders": [], "skipped": [], "editor": "vscode"},
  "open_with": {"media": "builtin", "pdf": "auto"},
  "integrations": {"cmdpal": false, "flow": false, "explorer_menu": false, "pipe_api": false},
  "device": "auto",
  "updates": {"auto_check": true},
  "launch_at_login": true,
  "portable": false
}
```

### 12.6 As built in M7 (2026-10-14)
- **Shape.** `app/src-tauri` is a workspace member; its binary is `trovara.exe` (5.4 MB release, about 30 MB of
  memory with its webviews idle). It finds `trovara-helper.exe` beside itself (or in `helper\`). Pages:
  `index.html` (panel), `search.html` (the window, the same `panel.js` in window mode), `setup.html`,
  `settings.html`, and `hotkey.js` (the shortcut recorder) shared by setup and settings.
- **Helpers.** `search-service` starts with the app. `index-service --watch --backfill --prune` runs the whole time
  with the policy flags from the settings (`--priority`, `--on-battery`, `--pause-fullscreen`, `--hours`,
  `--exclude`, `--exclude-ext`). The app no longer watches folders itself: the helper does (M6). Same arguments as
  the running helper's → it's told `sync`; other arguments → it's stopped (8 s to finish a file) and started again.
  A helper started before the model was there is started again when the download finishes. Pause/Resume goes over
  stdin and is kept in the settings. If the helper stops on its own (a crash, watchdog exit 75), it's started again
  after 3 s, at most 5 times in 10 minutes.
- **Job object.** Every helper is put in one job with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Verified: when
  `trovara.exe` is killed from Task Manager, its helpers end with it.
- **No priority from the app.** Helpers start at normal priority (only `CREATE_NO_WINDOW`), and the helper sets its
  own priority (§10.3b). DigUp's shell started them at `BELOW_NORMAL`, which is the 3–8× slowdown measured in M6.
- **Model.** `download-model --json` runs at every launch (a few ms when the files are there). Overall progress
  counts both files against 864,676,480 bytes. Setup and settings offer Try Again, and Use Files… (`--from`).
- **Hotkey.** `Control+Shift+Space` by default. The recorder turns a key press into an accelerator. It needs Ctrl,
  Alt or Win unless the key is F1–F24. `set_hotkey` lets go of the old shortcut, registers the new one, and puts
  the old one back if Windows refuses (another app holds it). While recording, the current shortcut is let go so
  pressing it reaches the recorder. Reset goes back to the default. The tray's Search… item shows the shortcut.
- **Open at.** `AssocQueryStringW(ASSOCSTR_EXECUTABLE)` finds the person's default app for the extension. When that
  app can be told where to start, Enter opens there:
  - VLC `--start-time=S`; mpv `--start=S`; MPC-HC/BE `/start ms`; PotPlayer `/seek=S`;
  - SumatraPDF `-page N`; Acrobat `/A page=N`;
  - Edge, Chrome, Brave, Firefox, Vivaldi, Opera `file:///…#page=N`.

  Anything else, Store apps included, opens the ordinary way. The person's choice of app always wins: Trovara never
  switches apps just to land on the moment. On the development PC, PDFs open in Brave and videos in VLC, both at the
  place.
- **Copy.** Ctrl+C sets `CF_HDROP` plus `Preferred DropEffect = COPY` through a message-only owner window. Opened
  without an owner, the clipboard drops the data, which was observed here. Paths lose any `\\?\` prefix. Verified
  with .NET's `GetFileDropList`. Ctrl+Shift+C copies the path as text. The field's own Ctrl+C still works while
  text is selected in it.
- **Search window.** Ctrl+O from the panel, or the tray. It has filter chips (All, Pictures, Screenshots, Documents,
  Video, Audio, Code), passed as the search's `kinds`, and up to 120 results. Pictures show in a grid with arrow
  keys moving across and down. The preview is 1600 px. Esc clears the query, then closes the window.
- **Setup.** Welcome → folders (with estimates) → shortcut (with the recorder) → fast catch-up (checked by default;
  `usn install` runs after setup closes, with one UAC prompt) → ready. Choose Folders… later shows only the
  folders page.
- **Settings.** Every change is saved at once:
  - shortcut; folders; folders left out; file types left out;
  - pause; on battery; fullscreen; hours; speed: Automatic, Fastest, Gentle or Most efficient
    (`auto`/`normal`/`low`/`eco`);
  - start at sign-in (`tauri-plugin-autostart`, `--background`, release builds only);
  - fast catch-up on or off (`usn install|uninstall`, each asks for UAC);
  - update check (stored only until M9); model state; Open Logs.
- **Files.** Settings are in `%LOCALAPPDATA%\Trovara\settings.json` (camelCase, written via a temporary file),
  and logs in `%LOCALAPPDATA%\Trovara\logs`. `TROVARA_DATA` moves both for dev runs.
- **Dev flags.** `--roots`, `--offer`, `--settings`, `--index`, `--models`, `--helper`, `--hotkey`, `--debug-show`,
  `--debug-setup` (click through setup while hidden), `--debug-query Q` (prints what the panel showed),
  `--debug-quit`, `--debug-open window[:Q]|settings|setup`.
- **Verified end to end.** With a fresh profile, hidden setup chose 2 folders, saved the settings, and indexed 17
  files in about 8 s (one 60-page PDF in two steps). The panel's query "receipt" then showed `scan-receipt.pdf`
  (page 1, preview drawn), 28 s from launch. A TIFF preview came out as a 1024×768 JPEG from `render`, with 4 of 4
  thumbnails drawn. Restarts catch up through the journal (`found: 0`, no walk). A killed index helper came back by
  itself.
- **Deferred.**
  - Acrylic/Mica: the panel uses a solid surface.
  - Tray icon states: status text and tooltip only.
  - Explorer's thumbnail cache.
  - Per-subfolder choices in setup.
  - Panel fade animation.
  - Code at a line: M8.
  - The update check itself: M9.

## 13. Optional integrations

### 13.1 Local search API (named pipe)
- `\\.\pipe\trovara-<user SID>`, created by the shell with a DACL granting only the current user SID
  (`ConvertStringSecurityDescriptorToSecurityDescriptorW("D:P(A;;GA;;;<SID>)")`), `PIPE_REJECT_REMOTE_CLIENTS`.
- It speaks the same JSON lines as search-service (`search`, `text`, `render` only). The shell proxies to
  search-service. Off by default.

### 13.2 PowerToys Command Palette extension
- C# (.NET 8) out-of-process extension per the Command Palette extension SDK (packaged with MSIX as PowerToys
  requires). It registers a "Trovara" provider and an optional keyword prefix.
- It talks to the named pipe. Results show name, kind, snippet and thumbnail (path from `render`). Commands: Open at
  place, Show in Explorer, Copy file.

### 13.3 Flow Launcher plugin
- C# plugin (`Flow.Launcher.Plugin`) or executable JSON-RPC plugin. Action keyword `dv`. Same pipe.

### 13.4 Explorer context menu
- Windows 11 top-level menu: `IExplorerCommand` COM server in `trovara-explorer.dll`, registered through an **MSIX
  sparse package** (identity without full MSIX install). Windows 10 / "Show more options": classic registry verbs.
- Verbs: on folders "Search in this folder with Trovara" (opens the window scoped to the folder), on files "Find similar
  with Trovara" (`similar` op).

### 13.5 Optional packs (download on request, verify signature + hash)
| Pack | Content | Location |
|---|---|---|
| CUDA | `ggml-cuda.dll`, cudart, cublas, cublasLt (matching CUDA 12.x) | `%LOCALAPPDATA%\Trovara\backends\cuda\` |
| ffmpeg | LGPL `ffmpeg.exe` + `ffprobe.exe` (a minimal build) | `…\tools\ffmpeg\` |
| libheif | `heif.dll` + `de265.dll` / `dav1d.dll` (LGPL/BSD) | `…\codecs\` |

Each pack is a signed zip on GitHub Releases. The manifest lists the SHA-256. Loaded only from that folder (no PATH
search, so no DLL planting): `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)` + `AddDllDirectory`.

## 14. Security and privacy

| Area | Requirement |
|---|---|
| Network | Only `huggingface.co` (+ CDN redirect), `github.com`/`objects.githubusercontent.com` (updates, packs). All HTTPS (rustls). No other hosts. Provide a "no network" switch, which disables updates and packs. |
| DLL loading | `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_APPLICATION_DIR \| LOAD_LIBRARY_SEARCH_SYSTEM32 \| LOAD_LIBRARY_SEARCH_USER_DIRS)` at process start in all binaries |
| Untrusted files | All decoding (PDF, images, media, docs) only in the helper (medium integrity, since it must read user files), which runs under a Job Object with memory and kill-on-close limits. Parsers are fuzzed in CI (`cargo fuzz` on rtf, doc, docx, html readers). |
| WebView | Strict CSP (as `shell/src-tauri/tauri.conf.json`). `asset:` scope only the indexed roots and previews. No remote content. `ask` command allowlist of ops (as `main.rs`). |
| Pipe API | Per-user DACL, reject remote clients, off by default |
| Updates | Tauri updater minisign signature check. Code signing on every exe/dll. |
| Data | Index in the user's profile (inherits user-only ACL). "Delete index" and "Clear logs" in Settings. Logs never include file content, only paths and queries, and that is stated in Settings → Privacy. |
| Secrets | Rules in §6.7. Applies in every index, every folder. |

## 15. Build, packaging and release

### 15.1 Build
- `scripts/build-llama.ps1`: clones and checks out the pinned llama.cpp commit, applies the patch, and runs CMake with
  MSVC (`-G "Visual Studio 17 2022" -A x64`), `-DBUILD_SHARED_LIBS=ON -DGGML_BACKEND_DL=ON -DGGML_CPU_ALL_VARIANTS=ON
  -DGGML_VULKAN=ON -DLLAMA_CURL=OFF -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF` (needs the Vulkan SDK at build
  time only). A second job builds `-DGGML_CUDA=ON` for the pack.
- `scripts/fetch-vendor.ps1`: PDFium `pdfium-win-x64` at `chromium/8086` (as DigUp's `fetch-vendor.sh`).
- `cargo build --release` with `[profile.release] lto = "fat", codegen-units = 1, opt-level = 3` for the helper
  (speed); `opt-level = "s"` for the shell (size, as DigUp's shell); `panic = "abort"`, `strip = true`. MSVC CRT linked
  dynamically (`/MD`, as the llama.cpp DLLs), with the VC++ runtime DLLs shipped app-local (§20.3).

### 15.2 Size budget (NFR-06)
| Item | Estimate |
|---|---|
| trovara.exe (shell + UI assets) | 6–9 MB |
| trovara-helper.exe | 6–10 MB |
| llama.dll + mtmd.dll + ggml-base.dll | 6–10 MB |
| ggml-cpu-* variants | 8–15 MB |
| ggml-vulkan.dll | 15–30 MB (shaders embedded) |
| pdfium.dll | 5–6 MB |
| **Installer (NSIS, LZMA)** | **≈ 30–55 MB** |

### 15.3 Installer
- Tauri NSIS bundle: per-user (`installMode: currentUser`), WebView2 bootstrapper (`webviewInstallMode:
  downloadBootstrapper`; offline variant embeds it), start-menu shortcut, uninstaller with a "Remove index and model"
  checkbox (F-UPD-04).
- Code signing: Azure Trusted Signing (or OV cert) via `signtool`, for every PE file, before NSIS packs them, then the
  installer itself.
- winget manifest (P1). MSIX (P2; also needed for the Explorer sparse package and the Command Palette extension).

### 15.4 Updates
- `tauri-plugin-updater` checks `latest.json` on GitHub Releases once a day (F-UPD-01) and on "Check Now". minisign
  public key compiled in. Download in the background, install on quit or on "Restart now".
- `vectorVersion` unchanged → the index is kept. Changed → the update notes say "re-reads your files", and the helper
  re-embeds in the background while old vectors keep serving search until each file is redone.

### 15.4b As built in M9 (2026-10-14)
- **One command.** `scripts\release.ps1 [-UpdateUrl <latest.json>] [-DownloadBase <URL>] [-Notes …]`:
  1. builds the helpers (release);
  2. stages `target\package`: the helpers, every DLL, the VC++ runtime app-local and Acknowledgements.txt;
  3. signs them (`scripts\sign.ps1`);
  4. runs `npx tauri build` (Tauri CLI 2.12.1, pinned in `app\package.json`);
  5. writes `latest.json`.
- **Installer.** It is **16.1 MB** (NFR-06 budget 60 MB). The installed folder is 81 MB: ggml-vulkan 44 MB, helper
  9.8 MB, trovara.exe 7.3 MB, pdfium 7.3 MB, 9 CPU variants of ~1.1 MB.
  - Per user, no admin needed. WebView2 comes from the bootstrapper when it's missing.
  - The VC++ runtime ships app-local: msvcp140, vcruntime140, vcruntime140_1 and vcomp140, read off the binaries'
    imports with dumpbin. The UCRT is part of Windows 10+.
- **Install folder.** `%LOCALAPPDATA%\Programs\Trovara`. Tauri's own default, `%LOCALAPPDATA%\<productName>`, is
  Trovara's data folder. So `app\src-tauri\nsis\installer.nsi` is Tauri 2.12.1's template, taken from the CLI, with
  that one line changed. The release script warns when the CLI moves on.
- **Hooks** (`nsis\hooks.nsh`). Uninstalling runs `trovara-helper usn uninstall --if-installed`, which asks for
  UAC only when the task runs this copy's reader. "Delete the application data" also removes
  `%LOCALAPPDATA%\Trovara` (F-UPD-04). Updates (`/UPDATE`) skip both. Tauri's uninstaller removes the sign-in entry.
- **Fast catch-up per copy.** `usn status` says "on" only when the task runs this copy's `trovara-usn.exe`. Another
  copy's task (a dev build) is "elsewhere (path)", and Settings offers Turn On, which rewrites the task (`/F`).
- **Updates.**
  - `tauri-plugin-updater` 2.13. The feed URL is baked in at build time (`TROVARA_UPDATE_URL`); builds without one
    never check.
  - The minisign public key is in `app\src-tauri\updater.pub` and tauri.conf. The private key and its password are
    in `%USERPROFILE%\.trovara`, and never in the repo.
  - The check runs daily, a minute after start, while the setting is on, or on demand from Settings → Check Now or
    the tray's "Check for Updates…".
  - The download is verified as it arrives and kept in memory. The tray then offers "Restart to Update to X";
    otherwise it installs when Trovara quits. The installer runs passive with `/UPDATE` and starts Trovara again with
    the same arguments.
- **Verified with a local feed** (`-AllowHttp -Version 1.0.0-beta.2`, test builds only):
  - An installed beta.1 found beta.2, downloaded and verified it, installed it, and restarted. The exe and the
    uninstall entry then read 1.0.0-beta.2.
  - With one byte of the update changed: "signature verification failed", and nothing was installed.
- **Verified install and uninstall.** A silent install into a scratch folder created the files, the uninstall entry
  and the Start-menu shortcut. The installed helper's self-check passed on the GPU. The installed app answered a
  debug query with the right result. The silent uninstall removed the program, its entry and its shortcut, and kept
  all 190 data files.
- **DLL hardening.** Every binary calls `SetDefaultDllDirectories(APPLICATION_DIR | SYSTEM32 | USER_DIRS)` first
  (TRD §14). GPU embedding is unchanged with it: 252 ms vs 246 ms, same vector, Vulkan still on the RTX 3050.
- **Acknowledgements.** `scripts\acknowledgements.ps1` (`cargo metadata`, normal dependencies only, Windows target)
  covers 372 crates and 245 distinct license texts, plus llama.cpp/ggml, PDFium and its licenses, the VC++ runtime,
  and the Gemma terms: 872 KB. It ships beside the app and opens from Settings → About → Licenses.
- **Signing.** `sign.ps1` uses a PFX, a store thumbprint (hardware tokens included) or Azure Trusted Signing, with an
  RFC 3161 timestamp. The staged files are signed before bundling; trovara.exe, the uninstaller and the installer
  through Tauri's `signCommand`. **No certificate yet:** this beta is unsigned, so SmartScreen will warn.
- **Not done yet.**
  - A code-signing certificate.
  - A GitHub repo for the release feed.
  - CI (§15.5).
  - winget (P1).
  - The uninstaller's "delete data" checkbox path, untested: silent uninstall can't tick it.

### 15.5 CI (GitHub Actions)
| Job | Runner | Steps |
|---|---|---|
| lint | windows-latest | `cargo fmt --check`, `cargo clippy -D warnings` |
| build-llama | windows-latest | Cached by commit + patch hash |
| test | windows-latest | `cargo test --workspace` (unit + reference vectors on CPU) |
| fuzz (nightly) | ubuntu-latest | `cargo fuzz` on the doc readers, 10 min each |
| eval (nightly / PR label) | self-hosted Windows + GPU | `trovara eval evals/*.json` on the testbed; fail on regression > 1 point vs baseline |
| package | windows-latest | Sign, NSIS, `latest.json`, upload to a draft release |

## 16. Testing

### 16.1 Unit and integration tests (port from DigUp's `Tests/`)
| DigUp test | Trovara test | Covers |
|---|---|---|
| `RulesTests` | `trovara-core/tests/rules.rs` | classification, secrets, datasets, machine names, MPEG-TS, Windows additions |
| `FolderSelectionTests` | `folder_selection.rs` | subfolder choices, `isInside`, case-insensitive paths, long paths, junctions |
| `IndexStoreTests` | `index_store.rs` | schema, renames by file ID, scope ranges, deletes cascade |
| `SearchTextTests` | `search_text.rs` | Bengali, Arabic normalization, snippets, prefix rules |
| `LongFilesTests` | `long_files.rs` | partial state, steps of 30 pages / 40 chunks, scale rows |
| `CodeTests` | `code.rs` | stretches, notebooks, .gitignore, name lookup focus line |
| `ImageTests` | `images.rs` | WIC decode, orientation, min side, sizing per budget (pictures from `Tests/DigUpKitTests/Pictures`) |
| `ReferenceVectorTests` | `trovara-llama/tests/reference.rs` | cosine ≥ 0.999 text/image/audio |
| (new) | `media.rs` | MF audio windows (count, loc), frame sampling + dedup, MPEG-TS |
| (new) | `ocr.rs` | screenshot text, scale limits |
| (new) | `power.rs`, `usn.rs` | event mapping (mocked) |
| (new) | `protocol.rs` | search-service and index-service JSON round trips |

### 16.2 Evals
- Port `scripts/make-testbed.sh` + `scripts/testbed/*` to `scripts/make-testbed.py` (Python + ffmpeg + Pillow), which
  writes `%USERPROFILE%\TrovaraTestbed` with the same files (synthetic images, videos, audio, multilingual docs, traps:
  duplicates, real MPEG-TS, datasets).
- `evals/synthetic.json`, `multilingual.json`, `code.json` used as-is (paths relative to `base`). An expected entry like
  `video.mp4@0-6` means the hit must point inside seconds 0–6.
- Metrics: R@1, R@5, MRR per tag. **Baseline = DigUp's numbers on a Mac**. Gate: no tag drops > 1 point.

### 16.3 Performance tests
- Bench with `criterion`: dot kernel (rows/s), FTS query, full `search` on 20k / 100k / 500k synthetic rows.
- An end-to-end script measures NFR-01…05 with ETW or `QueryPerformanceCounter` stamps in logs.
- Memory: idle tray and search-service private bytes after 10 min, with the encoder released.

### 16.4 Compatibility matrix
Windows 10 22H2, Windows 11 23H2/24H2/25H2 · NVIDIA (recent driver), AMD, Intel Arc/iGPU, no GPU · 8 GB / 16 GB RAM ·
NTFS, exFAT USB, ReFS (Dev Drive), network share (SMB: no watcher, so poll every 10 min), OneDrive folders.

## 17. Mapping: DigUp macOS → Trovara Windows

| DigUp (macOS) | Trovara (Windows) |
|---|---|
| SwiftUI menu bar app (`Sources/DigUpApp`) | Tauri 2 tray app (`app/`) |
| Carbon hotkey (`Hotkey.swift`) | RegisterHotKey (global-shortcut plugin) |
| NSPanel (`SearchPanel.swift`) | Frameless WebView2 window |
| Sparkle | tauri-plugin-updater |
| ImageIO | WIC (+ `image`, `resvg`, libheif pack) |
| Vision OCR | Windows.Media.Ocr |
| PDFKit | PDFium |
| AVFoundation | Media Foundation (+ ffmpeg pack) |
| `textutil` / NSAttributedString docs | zip+XML, RTF stripper, cfb, IFilter fallback |
| FSEvents | ReadDirectoryChangesW + USN journal |
| Screenshot xattr (`kMDItemIsScreenCapture`) | Name + folder heuristics |
| iCloud dataless check | Cloud placeholder attributes |
| Low Power Mode, `IOPS` battery | Battery/Energy Saver, power notifications |
| `nice`, QoS background | `PROCESS_MODE_BACKGROUND_BEGIN`, EcoQoS |
| Metal (llama.cpp) | Vulkan / CPU (+ CUDA pack) |
| Accelerate / vDSP dot | AVX2/F16C/AVX-512 intrinsics |
| Quick Look | In-panel preview |
| QuickTime at time (AppleScript) | Built-in player / VLC / mpv `--start` |
| Preview with search | SumatraPDF `-page -search` / Edge `#page=` |
| `NSWorkspace` reveal in Finder | `SHOpenFolderAndSelectItems` |
| `NSPasteboard` file | `CF_HDROP` |
| Login item | HKCU Run key |
| `~/Library/Application Support/DigUp` | `%LOCALAPPDATA%\Trovara` |
| DMG + notarization + Homebrew cask | NSIS + Authenticode + winget |

## 18. Work breakdown (milestones from PRD §12)

| M | Tasks |
|---|---|
| M0 | Workspace, CI, `build-llama.ps1` (CPU + Vulkan), `fetch-vendor.ps1`, logging, settings paths |
| M1 | C shim + FFI, `LlamaEmbedder` text, then image (port ImageSizing), then audio; reference tests; backend self-check; batching |
| M2 | Schema, `IndexStore`, `SearchText`, FTS, `Searcher` (meaning, words, fusion, strong count), SIMD kernel, `trovara search` CLI |
| M3 | WIC images + EXIF, docs readers + chunker, PDFium text + render, long-file steps |
| M4 | WinRT OCR for screenshots and scanned pages, multi-language engine choice |
| M5 | MF audio windows, MF video frames + dHash, soundtrack, MPEG-TS check |
| M6 | Rules (Windows), FolderSelection, crawler (file IDs, renames), watcher, USN catch-up, scheduler (power, hours), watchdog + Job Objects, estimator + calibration, `index-service` |
| M7 | `search-service` (+ render, Explorer thumbnail cache), shell: tray, hotkey, panel, window, onboarding, settings, actions (open-at, reveal, copy), model download UI |
| M8 | Code extractor, code index, `code:` routing, editor detection + open-at-line |
| M9 | NSIS, signing, updater, uninstall, Acknowledgements generation, beta |
| M10 | CUDA/ffmpeg/libheif packs, pipe API, Command Palette, Flow Launcher, Explorer menu, idle/fullscreen modes, portable mode, Find similar |
| M11 | ARM64 build (CPU + Vulkan on Snapdragon), xlsx/pptx, CJK word splitting, MSIX, winget |

## 19. Future and research
- **NPU:** ONNX Runtime with the DirectML or QNN execution provider for Copilot+ PCs. Needs an ONNX export of
  EmbeddingGemma 2 with vector parity; would be a second `Embedder` implementation behind the same trait.
- **CJK keywords:** `Windows.Data.Text.WordsSegmenter` before the FTS insert.
- **Linux:** the engine is platform-neutral by design (`trovara-core`); a `trovara-linux` crate (inotify, ffmpeg,
  Tesseract) would follow.

## 20. Open technical questions
1. ~~USN catch-up in v1?~~ Decided: yes, asked once with the scheduled-task design in §9.3.
2. ~~WebView2 `<video>` enough?~~ **Decided (2026-10-14): both, in two steps.**
   - **v1.0:** the panel plays what WebView2 plays (MP4/MOV/M4V with H.264 or AV1; WebM with VP8, VP9 or AV1;
     AAC, MP3, Opus, Vorbis, FLAC and WAV audio) and seeks to the moment. The helper checks the container and codecs
     with Media Foundation when it indexes and stores `info.web_playable`. For anything else (MKV, AVI, WMV, HEVC
     without the extension, MPEG-TS), the preview shows the frame at the moment (`render`), and Enter opens VLC/mpv at
     that second or the default player.
   - **v1.1 (P1):** a native player inside the panel, through Media Foundation's `IMFMediaEngine` rendering into a
     DirectComposition visual under the WebView. That plays every format Windows can play (MKV, HEVC, WMV, AVI) with
     exact seeking, and needs no ffmpeg. Note: MKV support in WebView2 is partial and isn't relied on.
3. ~~Static CRT?~~ **Decided: the dynamic CRT everywhere, with the VC++ runtime shipped app-local.** llama.cpp,
   mtmd and ggml are DLLs built `/MD`. If the Rust binaries linked the CRT statically, the process would have two
   CRTs (separate heaps, separate `FILE*`s), which is a real bug risk at the FFI boundary for no gain. So:
   - Rust builds with the default `/MD`.
   - The installer puts `vcruntime140.dll`, `vcruntime140_1.dll` and `msvcp140.dll` (about 1 MB, the redistributable
     license allows app-local copies) beside the executables. The UCRT is part of Windows 10/11.
   - That means no VC++ Redistributable installer and no admin rights.
4. ~~dHash threshold?~~ **Decided in M5: the reference's own rule.** An 8×8 *average* hash within 4 bits of the last
   kept frame means the same shot (§6.5b). It gave exactly one frame per shot on the test clip.
5. **Decided (2026-10-14): FFI without bindgen.** libclang isn't a build requirement. A small C shim
   (`crates/trovara-llama/shim/trovara_llama.c`), compiled by `cc` against the real headers, handles every call that
   takes or returns a struct by value or needs a parameter struct. Rust declares only the plain functions
   (`src/ffi.rs`). The reference-vector test catches any signature drift on a llama.cpp update.
