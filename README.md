<p align="center">
  <img src="assets/logo.png" alt="Trovara" width="128" height="128">
</p>

<h1 align="center">Trovara</h1>

<p align="center">
  <b>Describe it. Find it.</b><br>
  Private content search for your Windows files.
</p>

<p align="center">
  <a href="https://trovara.vercel.app/"><b>Website</b></a> ·
  <a href="https://github.com/Subhankar-Patra1/trovara/releases/latest"><b>Download</b></a>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D6" alt="Windows 10 and 11">
  <img src="https://img.shields.io/badge/status-beta-orange" alt="Beta">
  <img src="https://img.shields.io/badge/runs-100%25%20on%20your%20PC-success" alt="Runs on your PC">
</p>

https://github.com/user-attachments/assets/3298b45d-b659-4d0d-823d-150eeb81141c

<p align="center"><sub>The 75-second demo (1080p, with narration). Demo files are fictional.</sub></p>

Press **Ctrl+Shift+Space** from any app and describe what you remember. Trovara finds pictures, screenshots, PDFs,
documents, recordings, videos, and code in the folders you select. Results identify the matching page, moment, or
line. Opening at that location depends on the file type and the application that opens it.

It runs Google DeepMind's EmbeddingGemma 2 on your own PC (on your GPU when there is one). Nothing you index ever
leaves it.

[Why Trovara](#why-trovara) · [Install](#install) · [Supported files](#what-trovara-reads) ·
[How it works](#how-it-works) · [EmbeddingGemma 2](#how-embeddinggemma-2-helps) ·
[Code search](#code-search) · [Spotlight and Quick Look](#spotlight-and-quick-look) · [FAQ](#faq)

## Why Trovara?

You may remember what a file contains but forget its name or location. A dated screenshot name does not tell you
which error it shows. A long recording does not tell you where a topic starts. Trovara lets you search with those
details and inspect the matching content before opening the file.

- **Students and office workers:** find notes, receipts, slides, and passages in documents.
- **Photographers and video creators:** find pictures by their subjects and locate scenes or recorded topics.
- **Developers:** find code by its purpose or an identifier, then open the matching line in an editor.
- **Users who keep files local:** search selected folders without uploading the files or creating an account.

## What it finds

| You type | Trovara finds |
|---|---|
| `red car` | The photo of a red car, even if it's named `DSC_1001.png` |
| `payment declined` | The screenshot with those words on screen (read with Windows' own OCR) |
| `security deposit` | The page of the lease that says so, the words marked yellow |
| `the scene at the lake` | The moment in a video, and it plays from there |
| `what the teacher said about exams` | The minute of a recording where it's said |
| `code: refresh the access token` | The function that does it, opened at its line in your editor |

## Features

- **Search by meaning and by words at once.** Results appear as you type, and meaning results follow when you pause.
  A file's name counts too: `pan card` finds `PAN_Card_Front_View.jpeg`.
- **Supported file formats.** JPEG, PNG, HEIC, WebP and RAW photos. Screenshots and scanned PDFs are read with OCR. PDF,
  Word, Excel, PowerPoint, OpenDocument, RTF, EPUB, Markdown, emails (`.eml`, `.msg`), CSV. MP3, WAV, M4A. MP4,
  MKV and AVI, including their subtitles.
- **Previews that show why it matched.** The found words are marked yellow, on the PDF page itself, in screenshots,
  in documents and in code. Markdown is shown formatted and code is coloured.
- **A large viewer.** Press **Space**, or click the preview, to see the file large. Zoom with Ctrl + mouse wheel and
  turn PDF pages with ← / →.
- **Code search.** Add your repos, then search with `code:` by what the code does or by a name in it. It opens in
  VS Code, Cursor, Visual Studio, JetBrains IDEs, Sublime Text, Notepad++, Zed and others.
- **Stays up to date by itself.** New and changed files are read as they arrive. Indexing waits while you game or
  watch something fullscreen, and goes easy on battery.
- **Private by design.** No account and no cloud. The index lives in `%LOCALAPPDATA%\Trovara`.

## Install

1. Download `Trovara-Setup-<version>.exe` from [Releases](https://github.com/Subhankar-Patra1/trovara/releases).
2. Run it. It installs for you only, so no administrator rights are needed. On the last page, keep
   **Create desktop shortcut** ticked if you want an icon on the desktop.
3. Setup asks which folders to search, then downloads the model once (865 MB) and starts reading your files in the
   background. You can search right away: files are found by name until they've been read.

Windows 10 (22H2) or Windows 11, 64-bit, with 8 GB of RAM (16 GB recommended) and about 2 GB of free disk space.
A GPU with Vulkan can reduce indexing time. CPU-only operation is also supported.

## Keys

| Key | Does |
|---|---|
| **Ctrl+Shift+Space** | Open the search panel (you can change it in Settings) |
| **↑ / ↓** | Move between results |
| **Space** | View the result large |
| **Enter** | Open it, at the page or moment |
| **Ctrl+Enter** | Show it in Explorer |
| **Ctrl+C** | Copy the file, to paste into a chat, a mail or a folder |
| **Ctrl+O** | Open the larger search window, with filters by kind |
| **Tab** | Search the same words in your code |
| **Esc** | Close |

## What Trovara reads

Trovara reads supported files inside your selected folders. It searches different parts of each file type:

| File type | Examples | Searchable content |
|---|---|---|
| Photos and images | JPEG, PNG, GIF, BMP, TIFF, WebP, HEIC/HEIF, AVIF, SVG, supported RAW formats | Visual content, file names, and folder names |
| Screenshots | Supported image formats recognized as screenshots | Visual content and text read with optical character recognition (OCR) |
| PDFs | `.pdf`, including scanned pages | Page text, page images, and OCR text from scanned pages |
| Documents and saved email | TXT, Markdown, RTF, DOC/DOCX, ODT, HTML, EPUB, EML, MSG | Extracted text, divided into searchable passages |
| Spreadsheets and presentations | XLSX, XLSM, PPTX, CSV, TSV | Extracted cell or slide text; macros are not run |
| Audio | MP3, WAV, M4A, AAC, FLAC, OGG, and other supported audio formats | Audio segments with their timestamps |
| Video | MP4, MOV, MKV, AVI, WebM, and other supported video formats | Sampled frames, audio segments, and available subtitles |
| Optional code index | Python, JavaScript, TypeScript, Rust, C/C++, C#, Java, Go, PowerShell, notebooks, and other supported source formats | Source lines, notebook cells, and supported project documentation |

Video subtitle readers handle `.srt`, `.vtt`, and `.ass` files beside a video, plus supported embedded subtitles.
HEIC, AVIF, RAW, and some media formats depend on the Windows codecs installed on your PC. A recognized extension
does not guarantee that every file using that extension can be decoded. Password-protected or damaged files may
not be readable.

The full format lists are in [the file classification rules](crates/trovara-core/src/rules.rs).

### What it skips

Trovara skips hidden and system locations, common dependency and cache folders, recognized key or password files,
and cloud-only placeholder files. Code search also respects Git ignore rules and excludes common build output,
vendored code, and minified bundles. General file search does not include source code by default.

Folder selection determines which personal files you include. Use **Settings → Folders** to choose locations and
exclude folders or file types. Use **Settings → Code** to configure the separate code index.

## How it works

1. **Select folders.** Choose the local locations Trovara should search.
2. **Read supported files.** Background workers extract document text, read screenshot text, sample video frames,
   and divide audio and code into smaller parts. Each part keeps its page, timestamp, or line location.
3. **Build a local index.** Trovara stores searchable text and numerical content representations called
   **embeddings**. The index also records file paths and match locations. Your original files stay in place.
4. **Search by words and meaning.** Keyword search matches names, identifiers, and extracted text as you type.
   When typing pauses, the local model converts your description into an embedding. Trovara compares it with
   indexed content and combines the results with keyword matches.
5. **Inspect the match.** Results show a preview and the matching context. Press **Space** for the larger viewer,
   **Enter** to open the file, or **Ctrl+Enter** to locate it in Explorer.

New and changed files are indexed while the app runs. The first pass reads part of long files so other files can
become searchable sooner. Later passes process the remaining content. Search results can therefore be incomplete
while indexing continues.

## How EmbeddingGemma 2 helps

[EmbeddingGemma 2](https://ai.google.dev/gemma/docs/embeddinggemma/model_card_2) is Google DeepMind's multimodal
embedding model. It converts text, code, images, and audio into a shared numerical representation. Its native
output contains 768 numbers per embedding, and it supports more than 100 languages.

In Trovara, the model helps connect a description such as `red car` with the visual content of a photo. The filename
does not need to contain those words. The same retrieval process applies to document passages, audio segments,
video frames, and code. See [Google DeepMind's model overview](https://deepmind.google/models/gemma/embeddinggemma/)
for the model's intended search uses.

Trovara runs a quantized GGUF version locally through llama.cpp. Quantization stores model weights in a smaller
representation. The current download is about **865 MB**: a text model plus image and audio encoders. The download
uses pinned files and verifies their SHA-256 checksums.

The model supplies embeddings. Trovara's own readers, OCR, keyword index, ranking, and viewer turn those embeddings
into usable file search. It does not train a new model on your files or generate answers from them. Meaning matches
are estimates, so exact words and the preview help you confirm a result.

## Code search

Code search uses its own folders and index. Enable it when you want to search your projects:

1. Open **Settings → Code** and enable **Search code**.
2. Select **Add code folder…** and choose your project folders.
3. Choose an editor under **Open code in**.
4. Allow indexing to run, then type `code:` followed by a description or identifier.

| Query | Intended match |
|---|---|
| `code: refresh the access token` | Code that refreshes an authentication token |
| `code: retry a failed request with backoff` | Retry logic that waits between attempts |
| `code: refreshAccessToken` | A matching identifier, with definition detection where supported |

Trovara divides source files into stretches of lines and notebooks into cells. It indexes their text together
with the file's project path. It uses EmbeddingGemma 2's code-retrieval prompt for description searches, plus
keyword and identifier matching for names. Results retain line or cell locations.

Press **Enter** to open a result in the configured editor at the matching line. Definition detection can select
the defining line for an identifier. If no supported editor is available, Trovara uses the file's default
application. Line navigation depends on editor support.

The **Code** filter in the larger search window also selects the code index. When normal search shows a code-match
hint, **Tab** searches the same words in code. Code is read for search; Trovara does not run or modify it.
Turning off code search stops code indexing and removes the code index after confirmation. Your source files remain.

## Spotlight and Quick Look

These tools have different primary roles. Trovara combines file-content retrieval with previews on Windows.

| Tool | Platform | Primary role | Workflow |
|---|---|---|---|
| **Trovara** | Windows 10/11 | Search selected local files by words and content descriptions | Search, inspect the matching page/moment/line, then preview or open the result |
| **Spotlight** | macOS | System search for apps, files, and other information, plus actions | Enter a query, inspect results, then open an item or perform an action |
| **Quick Look** | macOS | Preview a selected file without opening its full application | Select a file, press Space, then inspect it; supported files also have markup or trimming tools |
| **QuickLook for Windows** | Windows | Space-bar file previews with format support and plugins | Select a file in Explorer or another supported location, then preview it |

Spotlight already includes previews and file-related actions. Trovara's focus is a Windows workflow for finding
content across selected file types, including audio/video segments and an optional code index. Trovara does not
replace macOS system actions, and its viewer does not provide Quick Look's editing tools.

For the current behavior, see Apple's [Spotlight guide](https://support.apple.com/guide/mac-help/search-with-spotlight-mchlp1008/mac),
[Quick Look guide](https://support.apple.com/guide/mac-help/view-and-edit-files-with-quick-look-mh14119/mac), and the
[QuickLook for Windows project](https://github.com/QL-Win/QuickLook).

## Privacy

Trovara goes online for three things only: to download the model once, to check for updates (you can turn this off),
and to fetch optional add-ons you ask for. Your files, your searches and the index never leave your PC.

## FAQ

<details>
<summary>How is Trovara different from Spotlight?</summary>

[Spotlight](https://support.apple.com/guide/mac-help/search-with-spotlight-mchlp1008/mac) searches apps, files, and
other information on macOS. It also provides previews and system actions. Trovara searches selected files on
Windows by exact words and content descriptions, including what images show and recordings contain. Results
identify the matching page, timestamp, or code line. Opening at that location depends on the external application.

</details>

<details>
<summary>How is Trovara different from Quick Look?</summary>

Quick Look previews a file you have already selected. Trovara first finds files from a search query, then provides
previews of the matching content. See [Spotlight and Quick Look](#spotlight-and-quick-look) for the platform comparison.

</details>

<details>
<summary>Why does setup download about 865 MB?</summary>

That is the EmbeddingGemma 2 model: about **310 MB** for the text model and **555 MB** for the image and audio
encoders. Setup downloads these files separately from the app installer. The model runs locally, so search does
not need a cloud AI service. Indexes and preview data require additional disk space.

</details>

<details>
<summary>Will Trovara slow down my PC?</summary>

The first indexing pass uses CPU, disk, and a supported GPU to read your selected files. After that, Trovara
processes new and changed files, plus any content left for later passes. Large files can still require more work.

Indexing runs in a helper process. It releases the model after an idle period but continues watching for changes.
The search text model loads when a meaning query needs it. The search service releases that model and its vectors
after five minutes without a query. Keyword search remains available. Pause indexing or adjust its limits in
**Settings → Indexing** if you need more resources for another task.

</details>

<details>
<summary>Does Trovara need an internet connection?</summary>

Setup downloads the model once. After the model is installed, indexing and search work offline. Update checks and
requested add-on downloads use the internet. You can disable update checks in Settings.

</details>

<details>
<summary>Do I need a GPU or a paid AI account?</summary>

No. The model can run on the CPU. A supported Vulkan GPU can accelerate indexing. Trovara does not require an AI
API key or a cloud account. See [Install](#install) for system requirements.

</details>

<details>
<summary>Why does a new file appear by name but not by its contents?</summary>

Its content may not have been indexed yet. Keyword filename matches can appear before content processing finishes.
Check the indexing status, wait for the file to be read, and search again. Long files may be processed in stages.

</details>

<details>
<summary>What should I check when a file does not appear?</summary>

Check that its folder is selected, the file is stored locally, and its format is supported. Check excluded folders
and file types. Confirm that indexing is not paused. For source files, enable code search and select the project
folder separately. Try an exact filename or a word inside the file before changing the description.

</details>

<details>
<summary>Can I search files stored in OneDrive or another cloud folder?</summary>

Locally downloaded files in selected folders can be indexed. Trovara skips cloud-only placeholders and does not
download them for you. Make the required files available locally through your cloud storage application first.

</details>

<details>
<summary>Which languages can I search?</summary>

EmbeddingGemma 2 supports more than 100 languages, including searches where the query and file use different
languages. Google states that performance varies between languages. See the
[model card](https://ai.google.dev/gemma/docs/embeddinggemma/model_card_2). The repository's multilingual search
examples cover English, Bengali, and Arabic; they do not verify all supported languages.

Text recognition uses Windows OCR and depends on the installed OCR language packs. A language supported by the
embedding model may not be supported by Windows OCR. For example, Windows OCR does not support Bengali.

Keyword search does not split text into individual words for languages written without spaces, such as Chinese,
Japanese, and Thai. This limits exact-word matching. Meaning search uses the embedding model and does not depend
on this word splitting.

</details>

<details>
<summary>Does Trovara move, edit, or upload my files?</summary>

Trovara reads files and writes its own local index and preview data. It does not move or edit your original files,
and it does not upload them. Opening a result hands the file to its associated application.

</details>

<details>
<summary>Where is the search index stored?</summary>

The default index folder is `%LOCALAPPDATA%\Trovara\index`. It contains searchable text, file paths, embeddings,
and match locations. Open **Settings → About** to see the active index location and access local logs.

</details>

<details>
<summary>Will Enter always open the exact PDF page or media timestamp?</summary>

The result records the matching location, but external applications differ in navigation support. Trovara passes
the page or timestamp when the application supports it. Otherwise, it opens the file normally. Use the built-in
preview or larger viewer to inspect the matched content.

</details>

<details>
<summary>Is Trovara a chatbot or a code assistant?</summary>

Trovara retrieves existing files and code. It does not write code, execute a project, or generate conversational
answers about documents. Code search helps you locate the relevant source so you can inspect it in your editor.

</details>

<details>
<summary>Can I pause indexing or change the selected folders?</summary>

Yes. Pause indexing from the tray or **Settings → Indexing**. Change searched locations under **Settings → Folders**.
Configure project folders separately under **Settings → Code**. Indexing settings also control battery behavior,
fullscreen pauses, and permitted hours.

</details>

<details>
<summary>Does Trovara run on macOS or Linux?</summary>

The current desktop app supports Windows 10/11. This repository does not provide macOS or Linux releases.

</details>

<details>
<summary>How do I remove Trovara?</summary>

Quit Trovara from its tray menu. Open Windows Settings, find Trovara in the installed apps list, and select
**Uninstall**. Select the uninstaller's option to delete application data if you also want to remove the model,
indexes, settings, and logs. Without this option, the data remains for a later installation.

The default data folder is `%LOCALAPPDATA%\Trovara`. Portable mode stores data in `Data` beside the executable.
If you set `TROVARA_DATA`, check that location for remaining data after uninstalling. Removing Trovara does not
delete the original files in your selected search folders.

</details>

## Build from source

You need Rust (stable, MSVC toolchain), Visual Studio 2022 or 2026 with "Desktop development with C++", Git, and
optionally the [Vulkan SDK](https://vulkan.lunarg.com/sdk/home#windows) for GPU support.

```powershell
scripts\build-llama.ps1     # llama.cpp at the pinned tag → vendor\llama (a few minutes)
scripts\fetch-vendor.ps1    # PDFium → vendor\pdfium
cargo build --release --workspace
target\release\trovara.exe
```

`vendor\` and `third_party\llama.cpp\` aren't in the repository: the two scripts above fetch and build them. Add
`-Cuda` to `build-llama.ps1` for the optional CUDA pack (needs the CUDA toolkit).

To build the installer you also need Node.js, for the Tauri CLI:

```powershell
scripts\release.ps1         # → target\release\bundle\nsis\Trovara-Setup-<version>.exe (~16 MB)
```

<details>
<summary>The command-line helper</summary>

```powershell
trovara-helper download-model
trovara-helper index $HOME\Pictures $HOME\Documents
trovara-helper search "a dog on the beach"
trovara-helper status
```

</details>

<details>
<summary>Project layout</summary>

| Path | What |
|---|---|
| `crates/trovara-core` | The engine: index, ranking, rules, code search |
| `crates/trovara-win` | Windows layer: WIC, OCR, Media Foundation, power, file IDs |
| `crates/trovara-extract` | Readers: PDF, Office, EPUB, email, subtitles, code |
| `crates/trovara-llama` | EmbeddingGemma 2 on llama.cpp |
| `crates/trovara-helper` | The background worker and the CLI |
| `crates/trovara-usn` | A small reader of the NTFS change journal (optional fast catch-up after a restart) |
| `app/` | The app (Tauri 2): tray, hotkey panel, search window, setup, settings |
| `scripts/` | Build and release scripts |
| `evals/` | Ranking evals |

</details>

## License

[MIT](LICENSE) © 2026 Subhankar Patra.

Trovara ships third-party components under their own licenses: see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md),
and Settings → About → Licenses in the app. The EmbeddingGemma 2 model isn't part of Trovara. It's downloaded on first
launch. Google lists EmbeddingGemma 2 under **Apache 2.0** in its
[official model card](https://ai.google.dev/gemma/docs/embeddinggemma/model_card_2).
