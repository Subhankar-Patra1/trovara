# PRD: Trovara for Windows

**Product Requirements Document** · v1.0 · 2026-10-13
**Product name:** Trovara (from Italian *trova*, "find")
**Status:** Independent project with its own name, branding and code base. Its feature set was specified by studying
an open-source (MIT) macOS app, DigUp 0.6.1, which is kept outside this project as a reference only. Where logic
is ported from it, its MIT notice is kept in `THIRD_PARTY_NOTICES.md`.
**Companion document:** [TRD.md](TRD.md), which covers how each requirement here is built

---

## 1. Summary

Trovara is a local, private, multimodal file search app for Windows 10/11. The user picks folders. Trovara reads the
pictures, screenshots, PDFs, documents, audio recordings, videos and (optionally) source code in them, and turns each
part into a vector with Google DeepMind's **EmbeddingGemma 2** model, running on the user's own PC. The user presses a
global hotkey, describes what they want in their own words, in any of 100+ languages, and Trovara opens the matching file
**at the right place**: the video at the moment, the PDF at the page, the podcast at the minute, the code at the line.

Exact words count too. Invoice numbers, error codes and names are matched as written, in file names, document text and
the text inside screenshots.

Trovara matches DigUp feature for feature, adds Windows-native integrations, and is built in Rust and C++ to be **fast,
small and light on the system**.

## 2. Problem

- Windows Search (taskbar and Explorer) finds files by **name, keyword and metadata only**. It can't find "the photo
  of a dog on the beach", "where they talk about sleep" in a podcast, or "the clause about pets" in a scanned lease.
- Semantic search on Windows exists only on Copilot+ PCs (NPU required), covers few file types, can't jump to a moment
  or page, and can't be used for code.
- Cloud tools (Google Photos, OneDrive search) need uploading private files.

## 3. Goals and non-goals

### Goals
| # | Goal |
|---|---|
| G1 | **Full feature parity** with DigUp for macOS (every feature in §6 marked P0) |
| G2 | **100% local.** No file content, file name or query ever leaves the PC. |
| G3 | **Fast:** keyword results on every keystroke under 30 ms, meaning results under 150 ms after typing pauses |
| G4 | **Small:** installer ≤ 60 MB (model downloaded separately, once). Tray app idle RAM ≤ 40 MB. |
| G5 | **Light:** indexing runs at background priority, respects battery and Battery Saver, never makes the PC feel slow |
| G6 | Works on **any GPU** (NVIDIA, AMD, Intel via Vulkan) and falls back to the CPU |
| G7 | **Optional extras** the user can turn on: CUDA, ffmpeg, launcher plugins, Explorer menu, portable mode and more (§7) |

### Non-goals (v1)
| # | Non-goal | Why |
|---|---|---|
| N1 | Putting results in the **taskbar search box** | Windows has no public API for third-party results there. Trovara has its own panel instead (§9). |
| N2 | Feeding Windows Search's indexer (IFilter or property handlers) | Replaces system handlers for common types and gives keyword-only results. Invasive, little gain. |
| N3 | Cloud sync, accounts, telemetry, sharing | Conflicts with G2 |
| N4 | Changing, moving, tagging or deleting user files | Trovara is read-only by design |
| N5 | Downloading cloud-only (OneDrive and similar) placeholder files | Would cost bandwidth and disk without consent |
| N6 | Chat or Q&A over files (RAG) | Out of scope for v1; maybe later |
| N7 | NPU acceleration | llama.cpp has no NPU backend; research item for v2 (TRD §19) |

## 4. Target users

| Persona | Needs |
|---|---|
| **Everyday user** (student, office worker) | Find "that screenshot of the payment error", "the lease PDF", "the photo from the beach trip" without remembering names |
| **Creator** (podcaster, video editor, photographer) | Jump to a moment in hours of recordings. Find shots by what they show. |
| **Multilingual user** | Search in English and find a note in Bengali, Arabic or Hindi, and the other way round |
| **Developer** | `code: retry a failed call with backoff` finds the function in their repos and opens it in VS Code at the line |
| **Privacy-conscious user** | Everything stays on the PC, and they can check that (no network except the model download and an optional update check) |

## 5. User stories

1. As a user, I install Trovara, pick my folders, and it tells me how long the first pass will take for each folder.
2. As a user, I press **Ctrl+Shift+Space** in any app, type "zebra in a video", press Enter, and the video opens at the
   moment the zebra appears.
3. As a user, I type "where they talk about sleep" and a podcast opens at that minute.
4. As a user, I type "the clause about pets in the lease" and see the PDF page with my words marked.
5. As a user, I type "payment declined error" and find the screenshot by the text inside it.
6. As a user, I type an invoice number like "INV-20931" and get the exact match first.
7. As a user, I search in English and find a document written in Bengali.
8. As a developer, I turn on code search for my repos, type `code: refreshAccessToken`, and my editor opens at the
   line that defines it.
9. As a laptop user, I tell Trovara to index only when plugged in, or only between 22:00 and 06:00.
10. As a user, I can pause indexing from the tray at any time and see how far it has got.
11. As a user, a file I save is searchable 1–2 seconds later.
12. As a user, I press Space on a result to preview it without opening another app.
13. As a user, I press Ctrl+Enter to see the file in Explorer, or Ctrl+C to copy the file and paste it anywhere.
14. As a power user, I see Trovara's results inside PowerToys Command Palette or Flow Launcher (optional).
15. As a user on a network that blocks Hugging Face, I download the model files elsewhere and point Trovara at them.
16. As a user, I uninstall Trovara and choose whether to remove its index and model.

## 6. Feature requirements

**Priority:** **P0** = needed for parity with DigUp, ships in v1.0 · **P1** = optional or new, ships in v1.x ·
**P2** = later.

### 6.1 Search panel (the main surface)
| ID | Requirement | Pri |
|---|---|---|
| F-PNL-01 | A global hotkey opens a floating search panel from any app. Default **Ctrl+Shift+Space** (decided). | P0 |
| F-PNL-02 | **The user can change the hotkey** in Settings and during setup, with a "press the keys you want" recorder: any modifiers (Ctrl, Alt, Shift, Win) plus a key, and a "Reset to Ctrl+Shift+Space" button. Trovara warns when the chosen key is taken (registration fails, or a known clash: PowerToys Run Alt+Space, Command Palette Win+Alt+Space, IME switch keys) and keeps the old key until a free one is chosen. | P0 |
| F-PNL-03 | The panel appears centred on the monitor with the mouse pointer, a little above the middle, and fades in and out. It shows in under 100 ms (window kept warm). | P0 |
| F-PNL-04 | Reopening shows the last query, selected, so typing replaces it | P0 |
| F-PNL-05 | Keyword results appear on **every keystroke** (the last word matches as a prefix) | P0 |
| F-PNL-06 | **Meaning results** replace them once typing pauses for 120 ms. The query is embedded while the user types (`embed_ahead`). | P0 |
| F-PNL-07 | Results are grouped: Screenshots, Images, Documents (PDF + docs), Video, Audio, Code; about 6 rows per group | P0 |
| F-PNL-08 | Results much weaker than the best are folded under "N weaker matches", which can be expanded | P0 |
| F-PNL-09 | Each row shows a thumbnail, the file name, why it matched (snippet with matched words marked, page number, or timestamp) and the folder | P0 |
| F-PNL-10 | A preview pane for the selected result: picture, PDF page, passage with words marked, video frame and player at the moment, audio player at the moment, code at the line | P0 |
| F-PNL-11 | "No good matches for '…'" state with tips, and an honest empty result when nothing stands out | P0 |
| F-PNL-12 | Status line: number of searchable files and folders. Indexing activity while it runs. | P0 |
| F-PNL-13 | Escape or a click outside closes the panel. The panel never steals focus once closed. | P0 |
| F-PNL-14 | Correct on multiple monitors, at 100–300% DPI scaling, and in light and dark themes (follows Windows) | P0 |
| F-PNL-15 | Fully usable with the keyboard alone and with Narrator (screen reader labels on rows and chips) | P0 |
| F-PNL-16 | Kind words in the query ("video", "pdf", "screenshot", "photo") boost that kind | P0 |

### 6.2 Keyboard actions
| ID | Key | Action | Pri |
|---|---|---|---|
| F-KEY-01 | ↑ ↓ | Move through results | P0 |
| F-KEY-02 | Enter | Open the result **at its place** (§6.4) | P0 |
| F-KEY-03 | Space | Preview the selected result (after the user has moved to a result) | P0 |
| F-KEY-04 | Ctrl+Enter | Show in Explorer, with the file selected | P0 |
| F-KEY-05 | Ctrl+C | Copy the **file** to the clipboard (pastes in Explorer, chat apps, mail) | P0 |
| F-KEY-06 | Ctrl+O | Open the same search in the big search window | P0 |
| F-KEY-07 | Esc | Close the panel, or the preview first if one is open | P0 |
| F-KEY-08 | Ctrl+Shift+C | Copy the file's path | P1 (new) |
| F-KEY-09 | Tab / Shift+Tab | Move between filter chips | P1 (new) |

### 6.3 Search window (Ctrl+O)
| ID | Requirement | Pri |
|---|---|---|
| F-WIN-01 | A resizable window with the same search, a **picture grid** for images, screenshots and video frames, and a list for the rest | P0 |
| F-WIN-02 | Filter chips: All, Screenshots, Images, Documents, Video, Audio, Code, each with its count | P0 |
| F-WIN-03 | Large preview with player controls; double-click opens the file | P0 |
| F-WIN-04 | "Trovara x.y is available" banner when an update is ready | P0 |
| F-WIN-05 | Code tab: "Code search is off" explainer with a button to turn it on, or "Search your code" tips | P0 |
| F-WIN-06 | Filters by date modified and folder | P1 (new) |
| F-WIN-07 | Sort by relevance or date | P1 (new) |

### 6.4 Opening results at their place
| ID | Kind | Behaviour | Pri |
|---|---|---|---|
| F-OPN-01 | Video | Plays in Trovara's built-in player at the matched second. "Open in…" uses VLC or mpv at that second when installed, otherwise the default app. | P0 |
| F-OPN-02 | Audio | Same as video, at the start of the matched 30 s window | P0 |
| F-OPN-03 | PDF | Opens at the matched page. With SumatraPDF: page + search for the words. With Edge: `#page=N`. With Acrobat: `/A page=N`. Otherwise the default app. | P0 |
| F-OPN-04 | Document | Opens in the default app. The preview shows the matched passage with words marked. | P0 |
| F-OPN-05 | Image / screenshot | Opens in the default app (Photos) | P0 |
| F-OPN-06 | Code | Opens in the chosen editor **at the line** (the definition for a name lookup). See §6.9 for the editor list. | P0 |
| F-OPN-07 | All | When the file is gone (moved or deleted since indexing), say so, and drop it from the index | P0 |

### 6.5 What Trovara reads
| ID | Content | Requirement | Pri |
|---|---|---|---|
| F-RD-01 | Pictures | JPEG, PNG, GIF, BMP, TIFF, WebP, HEIC/HEIF, AVIF, RAW (CR2, CR3, NEF, ARW, DNG, ORF, RAF, RW2…), SVG, ICO. Matched by **what they show**. Pictures smaller than 256 px on the short side are skipped. | P0 |
| F-RD-02 | Screenshots | Detected by name and folder (Snipping Tool, Win+PrtScn, ShareX, Greenshot, Lightshot, macOS names) and matched by **picture and text (OCR)** | P0 |
| F-RD-03 | PDFs | Page by page. Pages with little text (scans, slides) are embedded as images **and OCR'd** for keywords. Long PDFs: first 30 pages in the first pass, the rest after everything else. Up to 5,000 pages per file. | P0 |
| F-RD-04 | Documents | txt, md, rtf, doc, docx, odt, html/htm, in passages. Long documents read in steps. Up to about 8 million characters. | P0 |
| F-RD-05 | Number-heavy text | Text that is mostly numbers (data exports) is matched by exact words only, no vectors | P0 |
| F-RD-06 | Audio | Any length, in overlapping **30 s windows (25 s hop)**. A match points to its moment. mp3, m4a, aac, wav, flac, ogg, opus, wma. | P0 |
| F-RD-07 | Video | Any length. A frame every 3 s with near-duplicates dropped (about one per shot), **plus the soundtrack** as audio. mp4, mov, m4v, mkv, webm, avi, wmv, mts/m2ts/ts (real MPEG-TS only). | P0 |
| F-RD-08 | File and folder names | Always matched by keyword | P0 |
| F-RD-09 | Photo metadata | Date taken, size and rotation (EXIF) shown in results | P0 |
| F-RD-10 | Office formats xlsx, pptx | Text from slides and sheets | P1 (new) |
| F-RD-11 | Email files eml, msg | Subject and body | P2 |
| F-RD-12 | ePub | Chapters as documents | P2 |

### 6.6 What Trovara never reads (privacy rules)
| ID | Rule | Pri |
|---|---|---|
| F-PRV-01 | Only the folders the user chooses: whole folders, or only some of their subfolders | P0 |
| F-PRV-02 | Never keys, certificates, password or credential files (`.pem .key .pfx .p12 .kdbx .gpg .ppk`, `id_rsa*`, `.env*`, `*secret*`, `*credential*`, `.netrc`, `.npmrc`…) | P0 |
| F-PRV-03 | Never hidden or system folders, never `AppData`, `$Recycle.Bin`, `System Volume Information`, `Windows`, `Program Files`, `ProgramData`, caches, `node_modules`, `venv`, `__pycache__`… | P0 |
| F-PRV-04 | Inside a code project (a folder with `.git`, `package.json`, `Cargo.toml`, `*.sln`…) only **screenshots** are read, unless code search covers that folder | P0 |
| F-PRV-05 | Folders that look like **datasets** (300+ machine-named media files of one kind) are skipped and reported. Folders the user picked directly are never treated as datasets. | P0 |
| F-PRV-06 | **Never downloads cloud-only files** (OneDrive, Dropbox, Google Drive and iCloud for Windows placeholders) | P0 |
| F-PRV-07 | Read-only: never changes, moves, uploads or deletes user files | P0 |
| F-PRV-08 | The user can skip more folders and file types (`heic, txt`) in Settings and during setup | P0 |
| F-PRV-09 | Network access only for: model download (Hugging Face), daily update check (GitHub, can be turned off), optional add-on downloads. No telemetry, no account. | P0 |
| F-PRV-10 | Logs may contain file names and queries. They stay local, and Settings has a "Clear logs" button. | P0 |
| F-PRV-11 | Index files readable only by the user's Windows account (default per-user ACL in `%LOCALAPPDATA%`) | P0 |

### 6.7 Indexing
| ID | Requirement | Pri |
|---|---|---|
| F-IDX-01 | **Time estimate** per folder before indexing: files by kind, first-pass time, rest-of-long-files time, subfolders | P0 |
| F-IDX-02 | Per-PC **speed calibration**: estimates improve as indexing measures this machine | P0 |
| F-IDX-03 | First-pass order: screenshots → images → PDFs → docs → audio → video. The long-file remainder comes last. | P0 |
| F-IDX-04 | A new or changed file is searchable 1–2 s after it lands (live folder watching) | P0 |
| F-IDX-05 | Catch-up after Trovara was closed or the PC was off, without a full rescan. **v1 asks once** during setup: "Fast catch-up after restart needs administrator permission once." On Yes, one UAC prompt installs a small privileged reader for the NTFS change journal (USN). On No (or a non-admin account), a normal rescan is used. The choice can be changed in Settings → Indexing. | P0 |
| F-IDX-06 | Moved and renamed files keep their vectors (matched by file ID) | P0 |
| F-IDX-07 | Deleted files leave the index | P0 |
| F-IDX-08 | Pause and resume from the tray | P0 |
| F-IDX-09 | **Index on battery** on or off (default on, as in DigUp). Always pauses in Battery Saver / Energy Saver. | P0 |
| F-IDX-10 | **Index only at set hours** (from–to). Changes made in between are picked up when the window opens. | P0 |
| F-IDX-11 | Background priority: lower CPU, I/O and memory priority (EcoQoS on Windows 11). The user's apps always win. | P0 |
| F-IDX-12 | **Watchdog:** a file that takes over 15 minutes, or pushes memory past the limit, is stopped, marked unreadable, and indexing continues | P0 |
| F-IDX-13 | Removable and network drives: files on an unplugged drive neither fail nor count as waiting. They come back when it's plugged in. | P0 |
| F-IDX-14 | Folders that keep changing (build output, downloads in progress) don't stall indexing | P0 |
| F-IDX-15 | The index remembers which model and pipeline made its vectors. An update that leaves vectors unchanged keeps the index. One that changes them re-embeds. | P0 |
| F-IDX-16 | The model runs in a helper process that **quits when done**. The search side loads only while searching (text encoder about 310 MB, freed after idle). | P0 |
| F-IDX-17 | "Index now at full speed" button that drops background throttling until the queue is empty | P1 (new) |
| F-IDX-18 | **Index only while idle:** no keyboard or mouse input for N minutes | P1 (new) |
| F-IDX-19 | Pause while a fullscreen app or game runs, or while Focus / Do Not Disturb is on | P1 (new) |
| F-IDX-20 | Per-folder status: files done, waiting, skipped (with reasons), failed | P0 |

### 6.8 Model
| ID | Requirement | Pri |
|---|---|---|
| F-MDL-01 | First launch downloads EmbeddingGemma 2 Q8_0 GGUF (text 310 MB + image/audio encoders 555 MB = 865 MB) from Hugging Face at a pinned revision, **with progress, resume and SHA-256 check** | P0 |
| F-MDL-02 | Setup continues while the model downloads (pick folders meanwhile) | P0 |
| F-MDL-03 | **"Use Files…":** point Trovara at model files downloaded on another network. **"Copy Links"** copies the download URLs. | P0 |
| F-MDL-04 | Reuses a copy already in the Hugging Face cache (`%HF_HOME%`, `~/.cache/huggingface`) when its size and hash match | P0 |
| F-MDL-05 | Settings shows the model (Google · Apache 2.0 · 865 MB), its state, and a re-download button | P0 |
| F-MDL-06 | Automatic GPU use via Vulkan. CPU fallback with AVX2/AVX-512 auto-selection. | P0 |
| F-MDL-07 | Optional **CUDA pack** for NVIDIA (downloaded on request) | P1 |

### 6.9 Code search
| ID | Requirement | Pri |
|---|---|---|
| F-COD-01 | **Off by default.** Turned on in Settings → Code or during setup, for chosen folders. | P0 |
| F-COD-02 | Separate code index. Code never shows up in normal searches. | P0 |
| F-COD-03 | `code:` prefix searches the code index, by what the code does or by a name in it | P0 |
| F-COD-04 | A name lookup (`code: refreshAccessToken`) goes to the line that **defines** it, not to the callers | P0 |
| F-COD-05 | Reads source files (90+ languages), notebooks (cells, no outputs) and a repo's own docs and config (md, rst, yaml, toml) | P0 |
| F-COD-06 | Follows `.gitignore`. Skips vendored and generated code (`vendor`, `third_party`, `build`, `dist`, `target`, `obj`…), submodules and worktrees, minified files, licenses, data files (json, csv, lock, log), files over 1 MB, notebooks over 20 MB, secrets. | P0 |
| F-COD-07 | Each repo shows how long reading it takes and how much room its index needs | P0 |
| F-COD-08 | Re-reads changed code a minute after edits go quiet | P0 |
| F-COD-09 | "Open code in": VS Code, VS Code Insiders, Cursor, Windsurf, VSCodium, Zed, Sublime Text, JetBrains IDEs (IntelliJ, PyCharm, WebStorm, CLion, GoLand, RustRover, PhpStorm, Rider, RubyMine, Android Studio), **Notepad++**, **Visual Studio**, Neovim/Vim (in Windows Terminal), or the file's default app. Trovara picks an installed one, preferring one that's running. | P0 |
| F-COD-10 | "Turn Off Code Search…" deletes the code index after a confirmation | P0 |

### 6.10 Tray icon
| ID | Requirement | Pri |
|---|---|---|
| F-TRY-01 | Tray icon. The menu's first line says what's going on: "Indexing 1,240 of 5,300 · Photos", "Up to date · 12,431 files", "Paused", "Waiting for charger", "Downloading model 43%". | P0 |
| F-TRY-02 | Menu: Search… (with the hotkey), Pause/Resume Indexing, Settings…, Check for Updates, About, Quit | P0 |
| F-TRY-03 | Left-click opens the search panel | P0 |
| F-TRY-04 | Icon shows activity (indexing, paused, error) | P0 |
| F-TRY-05 | Quitting closes every helper process | P0 |

### 6.11 Onboarding (first run)
| ID | Step | Pri |
|---|---|---|
| F-ONB-01 | Welcome: "Describe it. Dig it up."-style intro. Three features (screenshots and photos, PDFs and documents, moments inside audio and video). A privacy note. The model download starts here. | P0 |
| F-ONB-02 | "What should Trovara search?": suggested folders (Pictures, Documents, Downloads, Desktop, Videos, Music, OneDrive) with **time estimates**. Folders can be expanded to pick subfolders. | P0 |
| F-ONB-03 | "Skip some folders or file types" (optional) | P0 |
| F-ONB-04 | "Search your code too?": off by default. Folder picker and editor choice. | P0 |
| F-ONB-04b | "Fast catch-up after restart?": explains the one-time administrator permission (F-IDX-05). Yes shows one UAC prompt. "Not now" uses normal rescans. | P0 |
| F-ONB-05 | "Open Trovara from anywhere": shows the hotkey (Ctrl+Shift+Space), lets the user **change it** with the key recorder, and try it right away | P0 |
| F-ONB-06 | "You're all set": open at login (on by default). Explains the tray icon. | P0 |
| F-ONB-07 | Each step shows "Step N of M" for screen readers | P0 |

### 6.12 Settings
| Section | Controls | Pri |
|---|---|---|
| General | Search shortcut · Open at login · Check for updates automatically (+ Check Now) · Theme (follow Windows / light / dark) | P0 |
| Folders | Folders to search (add, remove with file-count warning, choose subfolders) · Skip these folders · Skip these file types | P0 |
| Indexing | Fast catch-up after restart (on/off; turning on asks for admin once) · Index on battery · Index only at set hours (from–to) · **Index only while idle** (P1) · **Pause during fullscreen and games** (P1) · Background throttling on/off (P1) | P0/P1 |
| Model | State, size, license · Download · Use Files… · Copy Links · GPU in use (Vulkan device name, CPU) · **CUDA pack** (P1) | P0 |
| Code | Search my code · Code folders · Code skipped folders · Open code in · Turn Off Code Search… | P0 |
| Open results with | Video/audio player (built-in, VLC, mpv, default) · PDF viewer (SumatraPDF, Edge, Acrobat, default) | P1 (new) |
| Integrations | PowerToys Command Palette · Flow Launcher · Explorer context menu · Local search API (named pipe) | P1 (new) |
| Privacy | What's stored and where · Open index folder · Clear logs · Delete index (rebuild) | P0 |
| About | Version · Licenses (Acknowledgements) · Support link | P0 |

### 6.13 Updates, install, uninstall
| ID | Requirement | Pri |
|---|---|---|
| F-UPD-01 | Daily update check against GitHub Releases. Sends nothing about the user's files. Can be turned off. | P0 |
| F-UPD-02 | Signed updates, downloaded in the background, applied on next launch or on "Restart now" | P0 |
| F-UPD-03 | Installer: per-user, **no admin needed**, ≤ 60 MB, code-signed (no SmartScreen warning once reputation builds) | P0 |
| F-UPD-04 | Uninstaller asks whether to remove the model and index (`%LOCALAPPDATA%\Trovara`) | P0 |
| F-UPD-05 | `winget install Trovara` | P1 |
| F-UPD-06 | Scoop manifest, MSIX package (Microsoft Store) | P2 |

### 6.14 Command-line tool
| ID | Command | Pri |
|---|---|---|
| F-CLI-01 | `trovara estimate <folders> [--json]` | P0 |
| F-CLI-02 | `trovara index <folders> [--code] [--exclude …] [--exclude-ext …]` | P0 |
| F-CLI-03 | `trovara search "<query>" [--code] [--kinds …] [--limit n] [--json]` | P0 |
| F-CLI-04 | `trovara status [--json]` | P0 |
| F-CLI-05 | `trovara eval <evals/*.json>` (R@1, R@5, MRR) | P0 |
| F-CLI-06 | `trovara speed` (calibration) | P0 |
| F-CLI-07 | `trovara download-model` | P0 |

## 7. Optional components (user's choice)

All of these are **off or automatic by default**, and switched in Settings → Integrations, Model or Open results with.

| Option | Default | What it does | Cost |
|---|---|---|---|
| **CUDA backend pack** | Off | Faster indexing on NVIDIA GPUs | ~500 MB download |
| **ffmpeg fallback** | Off | Reads audio and video formats Media Foundation can't (some MKV, Opus, old AVI codecs) | ~40–80 MB download |
| **HEIC/AVIF decoder pack** (libheif) | Auto | For PCs without Microsoft's HEIF/HEVC extensions | ~5 MB download |
| **Code search** | Off | Separate code index (§6.9) | Index ~130 MB for llama.cpp-sized repos |
| **USN journal fast catch-up** | Asked once during setup | Instant catch-up after restart on NTFS drives | One UAC prompt |
| **Index only while idle** | Off | Indexes only when the user is away | — |
| **Pause during fullscreen and games** | On | No stutter in games and presentations | — |
| **PowerToys Command Palette extension** | Off | Trovara results inside Command Palette | small add-on |
| **Flow Launcher plugin** | Off | Trovara results inside Flow Launcher | small add-on |
| **Explorer context menu** | Off | "Search with Trovara" on folders, "Find similar" on images and docs | Windows 11 menu needs MSIX sparse package |
| **Open media in VLC/mpv** | Auto if installed | Opens at the exact second | — |
| **Open PDFs in SumatraPDF** | Auto if installed | Opens at the page with the words searched | — |
| **Local search API** | Off | Named pipe for scripts and other apps, current user only | — |
| **Portable mode** | Off | Runs from a folder or USB stick, data beside the exe | — |
| **Find similar** (query by file) | P1 | Right-click a result → find files that look or read like it | — |

## 8. Experience details

### 8.1 Panel layout
```
┌──────────────────────────────────────────────────────────────┐
│ 🔍  dog on the beach                                         │  56 px field
├──────────────────────────────┬───────────────────────────────┤
│ IMAGES                       │                               │
│ ▸ [thumb] IMG_2041.jpg       │        [ large preview ]      │
│   [thumb] beach-day.png      │                               │
│ VIDEO                        │   IMG_2041.jpg                │
│   [thumb] trip.mp4 · 1:42    │   Pictures\2025\Goa · 3.1 MB  │
│ DOCUMENTS                    │   Taken 12 Mar 2025           │
│   trip-notes.docx · "…dog…"  │                               │
│ ▾ 4 weaker matches           │                               │
├──────────────────────────────┴───────────────────────────────┤
│ 12,431 files · ↩ Open  Space Preview  Ctrl+↩ Explorer  Ctrl+O│
└──────────────────────────────────────────────────────────────┘
  780 px wide · 480 px tall with results
```

### 8.2 Opening flows
- Video/audio result → built-in player at the matched second (H.264/AAC/VP9/Opus/MP3 in WebView2). For formats the
  built-in player can't play: still frame + "Open in VLC at 1:42" or the default app.
- PDF result → preview shows the rendered page with matched words highlighted. Enter opens in the PDF viewer at the page.

## 9. Windows Search bar decision

| Question | Decision |
|---|---|
| Can results go into the taskbar search box? | **No.** No public API exists. Trovara uses **its own panel** on a hotkey, exactly as DigUp does on macOS (DigUp doesn't plug into Spotlight either; it runs next to it). |
| Integrate with other launchers? | **Yes, optional:** PowerToys Command Palette and Flow Launcher (§7) |
| Feed Windows Search's indexer? | **No** (non-goal N2) |
| Run next to Copilot+ semantic search? | Yes. No conflict. |

## 10. Non-functional requirements

| ID | Area | Target |
|---|---|---|
| NFR-01 | Panel show latency (hotkey → visible) | ≤ 100 ms |
| NFR-02 | Keyword results per keystroke | ≤ 30 ms (100k files) |
| NFR-03 | Meaning results after the pause (warm encoder) | ≤ 150 ms for 100k vectors on a 4-core CPU. ≤ 60 ms typical. |
| NFR-04 | Encoder cold start (text model, SSD) | ≤ 2 s |
| NFR-05 | New file → searchable | 1–2 s (image or short doc) |
| NFR-06 | Installer size | ≤ 60 MB (CPU + Vulkan) |
| NFR-07 | Tray app idle RAM | ≤ 40 MB. Search service idle without encoder ≤ 30 MB. |
| NFR-08 | Idle CPU | ~0% (event-driven, no polling) |
| NFR-09 | Indexing peak RAM | ≤ 3 GB typical. Watchdog limit min(8 GB, max(4 GB, RAM/2)). |
| NFR-10 | Index size | ~1.5 KB per vector + text. ~30 MB per 20k vectors. |
| NFR-11 | First pass | A few minutes for a few hundred files. About an hour for a big Downloads folder on a mid-range PC with a GPU. |
| NFR-12 | Search quality | No regression vs DigUp on the eval sets (synthetic, multilingual, code): R@1, R@5, MRR within 1 point |
| NFR-13 | Vector parity | Cosine ≥ 0.999 vs DigUp's reference vectors (text, image, audio) on CPU and Vulkan |
| NFR-14 | Stability | Crash-free sessions ≥ 99.5% in beta. A helper crash never takes down the tray app. |
| NFR-15 | OS support | Windows 10 22H2 and Windows 11 (x64). ARM64 in v1.x. |
| NFR-16 | Hardware | 8 GB RAM minimum (16 GB recommended). AVX2 CPU. GPU optional (Vulkan 1.2). ~2 GB free disk + index. |
| NFR-17 | Accessibility | Keyboard-only and Narrator. Respects Windows text size and high-contrast themes. |
| NFR-18 | Paths | Unicode names in any language (Bengali, Arabic, Korean, accents) and paths longer than 260 characters |

## 11. Success metrics (local only, no telemetry; measured in beta through opt-in feedback and evals)

- Eval scores equal to or better than DigUp's (NFR-12)
- Beta testers find what they were looking for in the top 5 in ≥ 85% of tries (feedback form)
- Median panel-to-open time ≤ 5 s
- Installer ≤ 60 MB, idle RAM ≤ 40 MB, verified on each release

## 12. Release plan

| Milestone | Content | Exit criteria |
|---|---|---|
| M0 Setup | Repo, CI, llama.cpp build on Windows | CI green |
| M1 Embedder | llama.cpp FFI + reference-vector parity | NFR-13 met on CPU and Vulkan |
| M2 Store + search CLI | SQLite schema, FTS5, ranking, `trovara search` | Ported unit tests pass |
| M3 Readers I | Images, documents, PDFs (text + render) | Evals for those tags pass |
| M4 OCR | Screenshots, scanned PDF pages | Screenshot evals pass |
| M5 Readers II | Audio, video (Media Foundation) | Audio/video evals pass |
| M6 Indexer | Crawler, rules, watcher, USN, scheduler, watchdog, power | F-IDX-* done |
| M7 App | Tray, panel, window, onboarding, settings (Tauri) | F-PNL, KEY, WIN, TRY, ONB, SET done |
| M8 Code search | Code index, `code:`, editors | Code evals pass |
| M9 Ship | Installer, updater, signing, uninstall | **v1.0 beta** |
| M10 Optional | CUDA pack, ffmpeg, launcher plugins, Explorer menu, idle and fullscreen modes, portable | v1.1 |
| M11 Later | ARM64, xlsx/pptx, Find similar, winget | v1.2+ |

## 13. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Vulkan vectors differ from Metal/CPU | Search quality | Reference-vector test per backend in CI. Fall back to CPU when a driver fails the self-check. |
| HEIC needs Microsoft's paid HEVC extension | HEIC photos unreadable | Optional libheif pack. Detect the codec at startup. |
| Hotkey clashes (IMEs, PowerToys) | Panel won't open | Detect a failed RegisterHotKey, show a warning, suggest alternatives |
| Antivirus flags an unsigned helper or DLLs | Install blocked | Code-sign every binary. Submit to Microsoft Defender. |
| WebView2 missing on old Windows 10 | UI won't load | Installer bootstraps the WebView2 runtime |
| OneDrive Files On-Demand reads trigger downloads | Bandwidth and privacy | Check attributes before opening. Never open placeholders. |
| Windows OCR language packs missing | Weaker screenshot keywords | Use installed languages. Settings links to "Add a language". |
| Large libraries (1M+ vectors) | RAM and latency | int8 vector scan + fp16 rerank. Optional HNSW (TRD §8.4). |

## 14. Open questions

**Decided (2026-10-13):**
- Name: **Trovara**. Independent project, with no other product's name in its folders, crates, binaries or UI.
- Hotkey: **Ctrl+Shift+Space** by default, which the user can change (F-PNL-02).
- Fast catch-up: v1 **asks once** for administrator permission, during setup, and the user can decline (F-IDX-05).

**Still open:**
1. Code-signing route: Azure Trusted Signing or an OV certificate?
2. Ship the CUDA pack from GitHub Releases or point users to NVIDIA's runtime?
3. Trovara's own license (MIT, Apache 2.0, proprietary…)?

## 15. Licensing and attribution

- Reference macOS app (DigUp, MIT): its copyright notice goes in `THIRD_PARTY_NOTICES.md` for any logic ported from it.
  It is not named in the product, UI or folder structure.
- EmbeddingGemma 2: Apache 2.0. Show the license in Settings → Model.
- llama.cpp (MIT), PDFium (BSD-3/Apache 2.0), Tauri (MIT/Apache 2.0), SQLite (public domain), other crates per Cargo
  licenses. Generated into `Acknowledgements.txt` at build time.
- Not affiliated with Google, Microsoft or Apple. Trademarks belong to their owners.
