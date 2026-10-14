//! What the indexer asks Windows about itself and the PC (TRD §9.1, §10.3, §10.4): a file's ID (so a move keeps its
//! vectors), the power state, background priority with EcoQoS, the process's memory, and idle time.

use std::path::Path;

use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{
    GetCurrentProcess, ProcessPowerThrottling, SetPriorityClass, SetProcessInformation,
    PROCESS_MODE_BACKGROUND_BEGIN, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

/// A file's identity: its volume's serial number and its NTFS file reference number. Stays the same through moves and
/// renames on the same volume. Opens the file for its attributes only (never its data, so a cloud placeholder isn't
/// downloaded).
pub fn file_id(path: &Path) -> Option<(u64, u64)> {
    unsafe {
        let handle: HANDLE = CreateFileW(
            &HSTRING::from(path.as_os_str()),
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
        .ok()?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        let result = GetFileInformationByHandle(handle, &mut info);
        let _ = CloseHandle(handle);
        result.ok()?;
        Some((
            info.dwVolumeSerialNumber as u64,
            ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
        ))
    }
}

/// The power source and savers right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Power {
    /// Running on the battery (unplugged).
    pub on_battery: bool,
    /// Battery Saver (Windows 10) or Energy Saver (Windows 11) is on.
    pub saver: bool,
    /// The battery's charge, 0–100, when there's a battery.
    pub percent: Option<u8>,
}

pub fn power() -> Power {
    let mut status = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut status) }.is_err() {
        return Power {
            on_battery: false,
            saver: false,
            percent: None,
        };
    }
    Power {
        // ACLineStatus 0 = offline (battery), 1 = online, 255 = unknown (desktops without a battery: treat as plugged in).
        on_battery: status.ACLineStatus == 0,
        // SystemStatusFlag 1 = Battery Saver / Energy Saver on.
        saver: status.SystemStatusFlag == 1,
        percent: (status.BatteryLifePercent <= 100).then_some(status.BatteryLifePercent),
    }
}

/// Runs this process in the background: lower CPU, I/O and memory priority, so the person's own programs always come
/// first. With `eco`, Windows 11's EcoQoS too (efficiency cores, lower clocks); GPU work isn't affected.
pub fn enter_background(eco: bool) {
    unsafe {
        let process = GetCurrentProcess();
        let _ = SetPriorityClass(process, PROCESS_MODE_BACKGROUND_BEGIN);
        if eco {
            let state = PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
                StateMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            };
            let _ = SetProcessInformation(
                process,
                ProcessPowerThrottling,
                &state as *const _ as *const _,
                std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            );
        }
    }
}

/// Below-normal CPU priority only: the person's programs get the processor first, while disk, memory and GPU work
/// keep their normal priority (background mode lowers all of them, and was 3.5× slower even on an idle PC).
pub fn set_below_normal(on: bool) {
    use windows::Win32::System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS};
    unsafe {
        let process = GetCurrentProcess();
        let _ = SetPriorityClass(
            process,
            if on {
                BELOW_NORMAL_PRIORITY_CLASS
            } else {
                NORMAL_PRIORITY_CLASS
            },
        );
        // Opt out of power throttling explicitly: on hybrid CPUs Windows 11 otherwise treats a below-normal process as
        // low QoS and keeps it on efficiency cores at low clocks (3× slower on an idle i5-13450HX). Below normal then
        // only means the person's programs go first.
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: Default::default(),
        };
        let _ = SetProcessInformation(
            process,
            ProcessPowerThrottling,
            &state as *const _ as *const _,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
}

/// Lowers (or restores) the calling thread only. `background`: Windows' thread background mode (lowest CPU and disk
/// priority); else below-normal CPU priority. The process keeps its normal priority class, which Windows also uses as
/// its GPU scheduling priority: lowering the whole process made every embedding wait behind the desktop compositor
/// (an audio window 0.11 → 1.38 s, a video 0.6 → 8.8 s, on an idle PC).
pub fn set_thread_low(on: bool, background: bool) {
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
        THREAD_MODE_BACKGROUND_END, THREAD_PRIORITY_BELOW_NORMAL, THREAD_PRIORITY_NORMAL,
    };
    unsafe {
        let thread = GetCurrentThread();
        let priority = match (on, background) {
            (true, true) => THREAD_MODE_BACKGROUND_BEGIN,
            (true, false) => THREAD_PRIORITY_BELOW_NORMAL,
            (false, true) => THREAD_MODE_BACKGROUND_END,
            (false, false) => THREAD_PRIORITY_NORMAL,
        };
        let _ = SetThreadPriority(thread, priority);
    }
}

/// Back to normal priority ("index now at full speed"): background mode and EcoQoS off.
pub fn leave_background() {
    use windows::Win32::System::Threading::PROCESS_MODE_BACKGROUND_END;
    unsafe {
        let process = GetCurrentProcess();
        let _ = SetPriorityClass(process, PROCESS_MODE_BACKGROUND_END);
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: Default::default(),
        };
        let _ = SetProcessInformation(
            process,
            ProcessPowerThrottling,
            &state as *const _ as *const _,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
}

/// Minutes after local midnight, now (for "index only at set hours").
pub fn local_minutes() -> u32 {
    let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    time.wHour as u32 * 60 + time.wMinute as u32
}

/// This process's private memory (what Task Manager shows as "Commit size"), in bytes.
pub fn memory_used() -> Option<u64> {
    unsafe {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
        .ok()?;
        Some(counters.PrivateUsage as u64)
    }
}

/// The PC's physical memory, in bytes.
pub fn physical_memory() -> u64 {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    if unsafe { GlobalMemoryStatusEx(&mut status) }.is_ok() {
        status.ullTotalPhys
    } else {
        8 << 30
    }
}

/// A fullscreen app, a game (Direct3D exclusive fullscreen) or a presentation has the screen, or Focus says busy:
/// the backlog waits (no stutter in games and slideshows).
pub fn fullscreen_busy() -> bool {
    use windows::Win32::UI::Shell::{
        SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE,
        QUNS_RUNNING_D3D_FULL_SCREEN,
    };
    match unsafe { SHQueryUserNotificationState() } {
        Ok(state) => {
            state == QUNS_BUSY
                || state == QUNS_RUNNING_D3D_FULL_SCREEN
                || state == QUNS_PRESENTATION_MODE
        }
        Err(_) => false,
    }
}

/// Seconds since the last keyboard or mouse input (for "index only while idle").
pub fn idle_seconds() -> f64 {
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    if unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        let now = unsafe { GetTickCount() };
        now.wrapping_sub(info.dwTime) as f64 / 1000.0
    } else {
        0.0
    }
}

#[allow(dead_code)]
fn filetime_seconds(time: FILETIME) -> f64 {
    let ticks = ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64;
    ticks as f64 / 1e7 - 11_644_473_600.0
}

/// DLLs load from the program's own folder and System32 only, never from the current folder or PATH (TRD §14): a DLL
/// planted beside a file being indexed, or in a folder on PATH, can't get into Trovara.
pub fn harden_dll_search() {
    use windows::Win32::System::LibraryLoader::{
        SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
        LOAD_LIBRARY_SEARCH_USER_DIRS,
    };
    // SAFETY: a process-wide setting with valid flags, made before anything else loads.
    unsafe {
        let _ = SetDefaultDllDirectories(
            LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_USER_DIRS,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_survive_a_rename() {
        let dir = std::env::temp_dir().join(format!("trovara-ids-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.txt");
        std::fs::write(&a, "x").unwrap();
        let before = file_id(&a).unwrap();
        let b = dir.join("b renamed.txt");
        std::fs::rename(&a, &b).unwrap();
        assert_eq!(file_id(&b), Some(before));
        assert!(file_id(&a).is_none());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(memory_used().unwrap() > 0);
        assert!(physical_memory() > 1 << 30);
        println!("power: {:?}, idle {:.0} s", power(), idle_seconds());
    }
}
