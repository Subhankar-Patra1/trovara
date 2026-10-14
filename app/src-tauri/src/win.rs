//! What the app does with Windows directly: files on the clipboard (Ctrl+C in the panel, then paste in Explorer, a
//! mail, a chat), and opening a result at its page or moment in the person's own app when that app can be told where.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Match the native window boundary to the panel's 18 px CSS corner radius.
pub fn fit_panel_frame(panel: &tauri::WebviewWindow) {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{HWND, RECT};
        use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE};
        use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn};
        use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

        let (Ok(handle), Ok(scale)) = (panel.hwnd(), panel.scale_factor()) else { return };
        let hwnd = HWND(handle.0);
        let mut bounds = RECT::default();
        let border = DWMWA_COLOR_NONE;
        // SAFETY: hwnd belongs to the panel. Each pointer refers to data that outlives its call.
        unsafe {
            // Windows 11 draws this border separately from the shadow. Windows 10 ignores this attribute.
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, (&border as *const u32).cast(),
                                          std::mem::size_of_val(&border) as u32);
            if GetWindowRect(hwnd, &mut bounds).is_err() {
                return;
            }
            let width = bounds.right - bounds.left;
            let height = bounds.bottom - bounds.top;
            if width <= 0 || height <= 0 {
                return;
            }
            let diameter = (36.0 * scale).round() as i32;
            let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, diameter, diameter);
            if region.is_invalid() {
                return;
            }
            // Windows owns the region after a successful call. Free it only if the call fails.
            if SetWindowRgn(hwnd, Some(region), true) == 0 {
                let _ = DeleteObject(region.into());
            }
        }
    }
    #[cfg(not(windows))]
    let _ = panel;
}

/// Puts files on the clipboard as Explorer does (CF_HDROP, marked "copy"): pasting makes copies, never moves them.
#[cfg(windows)]
pub fn copy_files(paths: &[String]) -> Result<(), String> {
    use windows::core::w;
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, POINT};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::UI::Shell::DROPFILES;
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE};

    const CF_HDROP: u32 = 15;
    if paths.is_empty() {
        return Ok(());
    }
    // DROPFILES, then each path in UTF-16 with its NUL, then one more NUL.
    let mut names: Vec<u16> = Vec::new();
    for path in paths {
        // Explorer's own form: no \\?\ prefix, backslashes.
        let path = path.strip_prefix(r"\\?\").unwrap_or(path).replace('/', "\\");
        names.extend(path.encode_utf16());
        names.push(0);
    }
    names.push(0);
    let header = std::mem::size_of::<DROPFILES>();

    /// Global memory holding `bytes`, for the clipboard to own once it's set.
    unsafe fn global(bytes: &[u8]) -> Result<HANDLE, String> {
        let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).map_err(|error| error.to_string())?;
        let target = GlobalLock(memory) as *mut u8;
        if target.is_null() {
            let _ = GlobalFree(Some(memory));
            return Err("can't lock memory for the clipboard".into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
        let _ = GlobalUnlock(memory);
        Ok(HANDLE(memory.0))
    }

    let drop_files = DROPFILES { pFiles: header as u32, pt: POINT::default(), fNC: false.into(), fWide: true.into() };
    let mut data = Vec::with_capacity(header + names.len() * 2);
    // SAFETY: DROPFILES is plain data; its bytes are read once, here.
    data.extend_from_slice(unsafe { std::slice::from_raw_parts(&drop_files as *const DROPFILES as *const u8, header) });
    data.extend(names.iter().flat_map(|unit| unit.to_le_bytes()));

    // SAFETY: the clipboard is opened, filled and closed here; memory set on it belongs to it afterwards, and memory
    // that wasn't set is freed.
    unsafe {
        // The clipboard needs an owner window: opened without one, what's set on it doesn't stay. A message-only
        // window serves; the data outlives it.
        let owner = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("Trovara clipboard"), WINDOW_STYLE(0), 0, 0, 0, 0,
                                    Some(HWND_MESSAGE), None, None, None)
            .map_err(|error| format!("can't reach the clipboard: {error}"))?;
        let mut opened = OpenClipboard(Some(owner));
        for _ in 0..10 {
            if opened.is_ok() {
                break;
            }
            // Another program has it open for a moment (clipboard managers do).
            std::thread::sleep(std::time::Duration::from_millis(20));
            opened = OpenClipboard(Some(owner));
        }
        if let Err(error) = opened {
            let _ = DestroyWindow(owner);
            return Err(format!("the clipboard is busy: {error}"));
        }
        let result = (|| {
            EmptyClipboard().map_err(|error| error.to_string())?;
            let files = global(&data)?;
            if let Err(error) = SetClipboardData(CF_HDROP, Some(files)) {
                let _ = GlobalFree(Some(HGLOBAL(files.0)));
                return Err(error.to_string());
            }
            let effect = RegisterClipboardFormatW(w!("Preferred DropEffect"));
            if effect != 0 {
                let copy = global(&1u32.to_le_bytes())?; // DROPEFFECT_COPY
                if SetClipboardData(effect, Some(copy)).is_err() {
                    let _ = GlobalFree(Some(HGLOBAL(copy.0)));
                }
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        let _ = DestroyWindow(owner);
        result
    }
}

#[cfg(not(windows))]
pub fn copy_files(_: &[String]) -> Result<(), String> {
    Err("copying files is only on Windows".into())
}

/// The program that opens this kind of file for the person (their default app), when it's an ordinary program.
#[cfg(windows)]
fn default_app(path: &Path) -> Option<PathBuf> {
    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_INIT_IGNOREUNKNOWN, ASSOCF_NOTRUNCATE, ASSOCSTR_EXECUTABLE};

    let extension = format!(".{}", path.extension()?.to_str()?);
    let mut buffer = vec![0u16; 1024];
    let mut length = buffer.len() as u32;
    // SAFETY: the buffer and its length are passed together and outlive the call.
    let result = unsafe {
        AssocQueryStringW(
            ASSOCF_NOTRUNCATE | ASSOCF_INIT_IGNOREUNKNOWN,
            ASSOCSTR_EXECUTABLE,
            &HSTRING::from(extension),
            None,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut length,
        )
    };
    if result.is_err() {
        return None;
    }
    let found = PathBuf::from(String::from_utf16_lossy(&buffer[..(length as usize).saturating_sub(1)]));
    // Store apps (Photos, Media Player) answer with a host program that can't be told where to start.
    found.is_file().then_some(found)
}

#[cfg(not(windows))]
fn default_app(_: &Path) -> Option<PathBuf> {
    None
}

/// Opens the file at a page (PDFs) or a moment (seconds into audio or video) in the person's default app, when that
/// app is one that can be told where (VLC, mpv, MPC, PotPlayer; SumatraPDF, Acrobat, Edge, Chrome, Firefox…). True:
/// it did; false: the app can't be told, so the caller opens the file the ordinary way.
pub fn open_at(path: &str, kind: &str, loc: f64) -> bool {
    let file = Path::new(path);
    let Some(app) = default_app(file) else { return false };
    let name = app.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_ascii_lowercase();
    let seconds = loc.max(0.0);
    let page = (loc.max(1.0)).trunc() as u64;
    let args: Vec<String> = match (kind, name.as_str()) {
        ("audio" | "video", "vlc.exe") => vec![format!("--start-time={seconds:.1}"), path.into()],
        ("audio" | "video", "mpv.exe") => vec![format!("--start={seconds:.1}"), path.into()],
        ("audio" | "video", n) if n.starts_with("mpc-hc") || n.starts_with("mpc-be") => {
            vec![path.into(), "/start".into(), format!("{}", (seconds * 1000.0) as u64)]
        }
        ("audio" | "video", n) if n.starts_with("potplayer") => vec![path.into(), format!("/seek={}", seconds as u64)],
        ("pdf", "sumatrapdf.exe") => vec!["-page".into(), page.to_string(), path.into()],
        ("pdf", "acrord32.exe" | "acrobat.exe") => vec!["/A".into(), format!("page={page}"), path.into()],
        ("pdf", "msedge.exe" | "chrome.exe" | "brave.exe" | "firefox.exe" | "vivaldi.exe" | "opera.exe") => {
            vec![format!("{}#page={page}", file_url(path))]
        }
        _ => return false,
    };
    Command::new(&app).args(&args).spawn().is_ok()
}

/// file:///C:/My%20Files/a.pdf
fn file_url(path: &str) -> String {
    let mut url = String::from("file:///");
    for byte in path.replace('\\', "/").bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => url.push(byte as char),
            other => url.push_str(&format!("%{other:02X}")),
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(file_url(r"C:\My Files\naïve #1.pdf"), "file:///C:/My%20Files/na%C3%AFve%20%231.pdf");
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    /// Puts Cargo.toml on the clipboard (check it with PowerShell's Get-Clipboard -Format FileDropList).
    #[test]
    #[ignore]
    fn copies_a_file() {
        let path = std::fs::canonicalize("Cargo.toml").unwrap().display().to_string();
        copy_files(&[path]).unwrap();
    }

    #[test]
    #[ignore]
    fn default_apps() {
        for extension in ["pdf", "mp4", "mkv", "mp3", "jpg"] {
            println!("{extension}: {:?}", default_app(Path::new(&format!("a.{extension}"))));
        }
    }
}

