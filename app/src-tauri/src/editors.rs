//! "Open code in" (PRD F-COD-09, TRD §12.3): the code editors installed here, found where their installers put them
//! (`%LOCALAPPDATA%\Programs`, Program Files, JetBrains Toolbox, Visual Studio's installer, PATH), and how each one
//! opens a file at a line. Trovara picks the person's choice, else an installed one that's running, else the first
//! installed one, else the file's default app.

use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

#[derive(Clone, Debug, Serialize)]
pub struct Editor {
    pub id: String,
    pub name: String,
    #[serde(skip)]
    pub exe: PathBuf,
    #[serde(skip)]
    style: Style,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Style {
    /// VS Code and its family: `-g "path:line"`.
    Goto,
    /// Zed, Sublime Text: `"path:line"`.
    Colon,
    /// JetBrains IDEs: `--line N "path"`.
    JetBrains,
    /// Notepad++: `-nN "path"`.
    NotepadPlusPlus,
    /// Visual Studio: `/Edit "path" /Command "Edit.GoTo N"`.
    VisualStudio,
    /// gVim: `+N "path"`.
    Plus,
    /// Neovim in Windows Terminal: `wt nvim +N "path"`.
    Terminal,
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The editors installed here, in the order Trovara prefers them.
pub fn installed() -> Vec<Editor> {
    let programs = env_path("LOCALAPPDATA").map(|p| p.join("Programs"));
    let program_files: Vec<PathBuf> =
        ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"].iter().filter_map(|v| env_path(v)).collect();
    let mut found: Vec<Editor> = Vec::new();
    let mut add = |id: &str, name: &str, style: Style, exe: PathBuf| {
        if exe.is_file() && !found.iter().any(|e| e.id == id) {
            found.push(Editor { id: id.into(), name: name.into(), exe, style });
        }
    };
    // The VS Code family and the editors that install per user.
    let per_user: &[(&str, &str, Style, &str)] = &[
        ("vscode", "VS Code", Style::Goto, r"Microsoft VS Code\Code.exe"),
        ("vscode-insiders", "VS Code Insiders", Style::Goto, r"Microsoft VS Code Insiders\Code - Insiders.exe"),
        ("cursor", "Cursor", Style::Goto, r"cursor\Cursor.exe"),
        ("windsurf", "Windsurf", Style::Goto, r"Windsurf\Windsurf.exe"),
        ("vscodium", "VSCodium", Style::Goto, r"VSCodium\VSCodium.exe"),
        ("zed", "Zed", Style::Colon, r"Zed\Zed.exe"),
        ("sublime", "Sublime Text", Style::Colon, r"Sublime Text\sublime_text.exe"),
        ("sublime", "Sublime Text", Style::Colon, r"Sublime Text 3\sublime_text.exe"),
        ("notepad++", "Notepad++", Style::NotepadPlusPlus, r"Notepad++\notepad++.exe"),
    ];
    for (id, name, style, place) in per_user {
        for base in programs.iter().chain(program_files.iter()) {
            add(id, name, *style, base.join(place));
        }
    }
    // JetBrains IDEs: Program Files\JetBrains\<IDE>\bin, or the Toolbox's %LOCALAPPDATA%\Programs\<IDE>\bin.
    const JETBRAINS: &[(&str, &str)] = &[
        ("idea64.exe", "IntelliJ IDEA"), ("pycharm64.exe", "PyCharm"), ("webstorm64.exe", "WebStorm"),
        ("clion64.exe", "CLion"), ("goland64.exe", "GoLand"), ("rustrover64.exe", "RustRover"),
        ("phpstorm64.exe", "PhpStorm"), ("rider64.exe", "Rider"), ("rubymine64.exe", "RubyMine"),
        ("studio64.exe", "Android Studio"),
    ];
    let mut jetbrains_folders: Vec<PathBuf> = Vec::new();
    for base in program_files.iter().map(|p| p.join("JetBrains")).chain(programs.clone()) {
        if let Ok(entries) = std::fs::read_dir(&base) {
            jetbrains_folders.extend(entries.flatten().map(|e| e.path().join("bin")));
        }
    }
    jetbrains_folders.extend(program_files.iter().map(|p| p.join(r"Android\Android Studio\bin")));
    for (exe, name) in JETBRAINS {
        for folder in &jetbrains_folders {
            add(exe.trim_end_matches("64.exe"), name, Style::JetBrains, folder.join(exe));
        }
    }
    // Visual Studio, through its installer.
    if let Some(devenv) = visual_studio() {
        add("visualstudio", "Visual Studio", Style::VisualStudio, devenv);
    }
    // Vim and Neovim.
    for base in &program_files {
        if let Ok(entries) = std::fs::read_dir(base.join("Vim")) {
            for entry in entries.flatten() {
                add("gvim", "gVim", Style::Plus, entry.path().join("gvim.exe"));
            }
        }
        add("neovim", "Neovim", Style::Terminal, base.join(r"Neovim\bin\nvim.exe"));
    }
    if let Some(nvim) = on_path("nvim.exe") {
        add("neovim", "Neovim", Style::Terminal, nvim);
    }
    found
}

/// The editor to open code in: `chosen` (an id) when installed, else one that's running, else the first installed.
pub fn pick(chosen: &str) -> Option<Editor> {
    let installed = installed();
    if let Some(editor) = installed.iter().find(|e| e.id == chosen) {
        return Some(editor.clone());
    }
    let running = running_programs();
    installed
        .iter()
        .find(|e| {
            e.exe.file_name().is_some_and(|name| running.contains(&name.to_string_lossy().to_ascii_lowercase()))
        })
        .or(installed.first())
        .cloned()
}

/// Opens `path` in `editor`, at `line` when given.
pub fn open(editor: &Editor, path: &str, line: Option<u64>) -> Result<(), String> {
    command(editor, path, line).spawn().map(|_| ()).map_err(|error| format!("can't start {}: {error}", editor.name))
}

/// The command that opens `path` in `editor` at `line`.
fn command(editor: &Editor, path: &str, line: Option<u64>) -> Command {
    match editor.style {
        Style::Terminal => {
            let mut command = match on_path("wt.exe").or_else(windows_terminal) {
                Some(wt) => {
                    let mut command = Command::new(wt);
                    command.arg(&editor.exe);
                    command
                }
                None => {
                    let mut command = Command::new("cmd");
                    command.args(["/c", "start", ""]).arg(&editor.exe);
                    command
                }
            };
            if let Some(line) = line {
                command.arg(format!("+{line}"));
            }
            command.arg(path);
            command
        }
        style => {
            let mut command = Command::new(&editor.exe);
            match (style, line) {
                (Style::Goto, Some(line)) => command.arg("-g").arg(format!("{path}:{line}")),
                (Style::Colon, Some(line)) => command.arg(format!("{path}:{line}")),
                (Style::JetBrains, Some(line)) => command.arg("--line").arg(line.to_string()).arg(path),
                (Style::NotepadPlusPlus, Some(line)) => command.arg(format!("-n{line}")).arg(path),
                (Style::VisualStudio, Some(line)) => {
                    command.arg("/Edit").arg(path).arg("/Command").arg(format!("Edit.GoTo {line}"))
                }
                (Style::VisualStudio, None) => command.arg("/Edit").arg(path),
                (Style::Plus, Some(line)) => command.arg(format!("+{line}")).arg(path),
                _ => command.arg(path),
            };
            command
        }
    }
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|folder| folder.join(name)).find(|p| p.is_file())
}

/// Windows Terminal's alias (an app execution alias, a zero-byte file `is_file` sees through).
fn windows_terminal() -> Option<PathBuf> {
    let alias = env_path("LOCALAPPDATA")?.join(r"Microsoft\WindowsApps\wt.exe");
    alias.exists().then_some(alias)
}

/// Visual Studio's devenv.exe, as its installer reports the newest one.
fn visual_studio() -> Option<PathBuf> {
    let vswhere = env_path("ProgramFiles(x86)")?.join(r"Microsoft Visual Studio\Installer\vswhere.exe");
    if !vswhere.is_file() {
        return None;
    }
    let mut command = Command::new(vswhere);
    command.args(["-latest", "-property", "productPath", "-utf8"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().ok()?;
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    (path.file_name().is_some_and(|n| n.eq_ignore_ascii_case("devenv.exe")) && path.is_file()).then_some(path)
}

/// The programs running now, by file name (lowercase).
#[cfg(windows)]
fn running_programs() -> std::collections::HashSet<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let mut names = std::collections::HashSet::new();
    // SAFETY: a process snapshot read entry by entry into a struct sized for it, then closed.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return names };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let length = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            names.insert(String::from_utf16_lossy(&entry.szExeFile[..length]).to_ascii_lowercase());
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    names
}

#[cfg(not(windows))]
fn running_programs() -> std::collections::HashSet<String> {
    Default::default()
}

/// The display name for a chosen id, for the panel's button ("Open in VS Code").
pub fn name_of(editor: Option<&Editor>) -> String {
    editor.map_or_else(|| "its app".to_string(), |e| e.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_editor_at_its_line() {
        let args = |style: Style, line: Option<u64>| {
            let editor = Editor { id: "x".into(), name: "X".into(), exe: PathBuf::from("x.exe"), style };
            command(&editor, "a.py", line).get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ")
        };
        assert_eq!(args(Style::Goto, Some(21)), "-g a.py:21");
        assert_eq!(args(Style::Goto, None), "a.py");
        assert_eq!(args(Style::Colon, Some(21)), "a.py:21");
        assert_eq!(args(Style::JetBrains, Some(21)), "--line 21 a.py");
        assert_eq!(args(Style::NotepadPlusPlus, Some(21)), "-n21 a.py");
        assert_eq!(args(Style::VisualStudio, Some(21)), "/Edit a.py /Command Edit.GoTo 21");
        assert_eq!(args(Style::Plus, Some(21)), "+21 a.py");
        assert!(args(Style::Terminal, Some(21)).ends_with("x.exe +21 a.py"));
    }

    /// What this PC has (a look, not a check: every PC differs).
    #[test]
    #[ignore]
    fn editors_here() {
        for editor in installed() {
            println!("{} ({}): {}", editor.name, editor.id, editor.exe.display());
        }
        println!("picked: {:?}", pick("").map(|e| e.name));
    }
}
