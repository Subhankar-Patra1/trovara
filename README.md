# Trovara

**Describe it. Find it.** Local, private search for the files on your Windows PC by what's in them: pictures,
screenshots, PDFs, documents, recordings, videos and code. It runs on Google DeepMind's EmbeddingGemma 2, entirely on
your PC.

Press **Ctrl+Shift+Space** (you can change it), describe what you're looking for in any language, and Trovara opens
the match at the right place: the video at the moment, the PDF at the page, the code at the line.

> Status: early development. Done: M0 (build setup), M1 (EmbeddingGemma 2 on llama.cpp, GPU via Vulkan, vectors
> matching the reference at cosine ≥ 0.9998), M2 (SQLite index, hybrid search: 26 ms at 100k vectors), M3 (pictures in every Windows format, PDFs, Word/OpenDocument/RTF/HTML/legacy .doc), M4 (OCR of screenshots and scanned pages), M5 (audio and video at the moment), M6 (background indexer: rules, moves, live watching, power rules, watchdog, estimates, opt-in NTFS fast catch-up), M7 (the app: tray, hotkey panel, search window, setup, settings), M8 (code search: `code:` finds code by what it does or by a name, opening at the defining line in your editor), M9 (a 16 MB per-user installer, signed updates, uninstall). See
> [docs/PRD.md](docs/PRD.md) for what it will do and [docs/TRD.md](docs/TRD.md) for how.
>
> Try it:
> ```powershell
> trovara-helper download-model
> trovara-helper index $HOME\Pictures $HOME\Documents   # pictures, PDFs, documents, audio, video
> trovara-helper search "a dog on the beach"
> ```
> Or the app: `cargo build --release --workspace`, then run `target\release\trovara.exe`. Setup opens on the
> first launch; after that, Ctrl+Shift+Space opens the search panel from any app.
> Or the app: `cargo build --release --workspace`, then run `target
elease	rovara.exe`. Setup opens on the
> first launch; after that, Ctrl+Shift+Space opens the search panel from any app.

## Layout

| Path | What |
|---|---|
| `crates/trovara-core` | Engine: kinds, options, prompts, model pins, paths (and later the crawler, index, search) |
| `crates/trovara-win` | Windows platform layer: WIC, OCR, Media Foundation, power, file IDs, shell actions |
| `crates/trovara-extract` | Readers: PDF, documents, code |
| `crates/trovara-llama` | EmbeddingGemma 2 on llama.cpp |
| `crates/trovara-helper` | The worker process and CLI |
| `crates/trovara-usn` | Small elevated NTFS change-journal reader (opt-in fast catch-up) |
| `app/` | The app (Tauri 2): tray, hotkey panel, search window, setup, settings |
| `scripts/` | Build scripts |
| `evals/` | Ranking evals |
| `docs/` | PRD and TRD |

## Build

You need:
- Rust (stable, MSVC toolchain)
- Visual Studio 2022 or 2026 with "Desktop development with C++"
- Git
- Optionally the Vulkan SDK, for GPU support

```powershell
scripts\build-llama.ps1     # llama.cpp at the pinned tag → vendor\llama (a few minutes)
scripts\fetch-vendor.ps1    # PDFium → vendor\pdfium
cargo build --release
cargo test --workspace
target\release\trovara-helper.exe version
```

Install the [Vulkan SDK](https://vulkan.lunarg.com/sdk/home#windows) before running `build-llama.ps1` to build the GPU
backend. Add `-Cuda` for the optional CUDA pack (needs the CUDA toolkit).

## Release

You also need Node.js (for the Tauri CLI, installed into `app\node_modules` on first use).

```powershell
scripts\release.ps1                      # → target\release\bundle\nsis\Trovara_<version>_x64-setup.exe (~16 MB)
scripts\release.ps1 -UpdateUrl https://github.com/Subhankar-Patra1/trovara/releases/latest/download/latest.json `
                    -DownloadBase https://github.com/Subhankar-Patra1/trovara/releases/download/v<version>
```

- **Updates:** builds with `-UpdateUrl` check that feed daily. Upload the installer, its `.sig` and `latest.json` to
  the release. The signing key is `%USERPROFILE%\.trovara\updater.key`, with its password beside it. Keep both
  private and backed up: without them no update can be shipped to existing installs.
- **Code signing:** set `TROVARA_SIGN_PFX` (+ `TROVARA_SIGN_PASSWORD`), `TROVARA_SIGN_THUMBPRINT`, or
  `TROVARA_SIGN_DLIB` + `TROVARA_SIGN_METADATA` (Azure Trusted Signing). Without one the build is unsigned.
- **Licenses:** `scripts\acknowledgements.ps1` regenerates `Acknowledgements.txt`. The release script runs it.

## Privacy

Nothing you index leaves your PC. Trovara goes online only to download the model once, to check for updates (which you
can turn off), and to fetch optional add-ons you ask for.

## License

[MIT](LICENSE) © 2026 Subhankar Patra. Trovara ships third-party components under their own licenses; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) and the `Acknowledgements.txt` beside the installed app (Settings →
About → Licenses). The EmbeddingGemma 2 model isn't part of Trovara: it's downloaded on first launch under Google's
[Gemma Terms of Use](https://ai.google.dev/gemma/terms).
