//! Win32 process, thread, window and file-lock helpers.

use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use windows::core::{BOOL, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_MORE_DATA, ERROR_SUCCESS, FILETIME, HANDLE, HLOCAL, HWND, LPARAM,
    NTSTATUS, POINT,
};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::System::Diagnostics::Debug::{
    MiniDumpWithFullMemory, MiniDumpWithHandleData, MiniDumpWithThreadInfo,
    MiniDumpWithUnloadedModules, MiniDumpWriteDump,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::RestartManager::{
    RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY,
    RM_PROCESS_INFO,
};
use windows::Win32::System::Threading::{
    GetPriorityClass, GetProcessAffinityMask, GetThreadDescription, GetThreadTimes, OpenProcess,
    OpenThread, ResumeThread, SetPriorityClass, SetProcessAffinityMask, SuspendThread,
    TerminateThread, ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, HIGH_PRIORITY_CLASS,
    IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_ACCESS_RIGHTS, PROCESS_DUP_HANDLE,
    PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    PROCESS_SUSPEND_RESUME, PROCESS_VM_READ, REALTIME_PRIORITY_CLASS, THREAD_ACCESS_RIGHTS,
    THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME, THREAD_TERMINATE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetCursorPos, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, WindowFromPoint, GA_ROOT,
};

use super::{Locker, ThreadAction, ThreadInfo, WindowInfo};
use crate::platform::Priority;

// ntdll exports these but the `windows` crate doesn't bind them.
windows_link::link!("ntdll.dll" "system" fn NtSuspendProcess(process: HANDLE) -> NTSTATUS);
windows_link::link!("ntdll.dll" "system" fn NtResumeProcess(process: HANDLE) -> NTSTATUS);

/// Owned kernel handle, closed on drop.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: we own this handle and close it exactly once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

fn friendly(e: windows::core::Error, what: &str) -> String {
    let code = e.code().0 as u32;
    match code {
        0x8007_0005 => {
            format!("Access denied while trying to {what} (try running as administrator)")
        }
        0x8007_0057 => "That process has already exited".into(),
        _ => format!("Could not {what}: {}", e.message()),
    }
}

fn open_process(pid: u32, access: PROCESS_ACCESS_RIGHTS, what: &str) -> Result<Owned, String> {
    // SAFETY: OpenProcess has no memory-safety preconditions; the handle is owned by `Owned`.
    unsafe { OpenProcess(access, false, pid) }
        .map(Owned)
        .map_err(|e| friendly(e, what))
}

fn open_thread(tid: u32, access: THREAD_ACCESS_RIGHTS, what: &str) -> Result<Owned, String> {
    // SAFETY: as above, for a thread handle.
    unsafe { OpenThread(access, false, tid) }
        .map(Owned)
        .map_err(|e| friendly(e, what))
}

fn nt_ok(status: NTSTATUS, what: &str) -> Result<(), String> {
    if status.0 >= 0 {
        Ok(())
    } else if status.0 as u32 == 0xC000_0022 {
        Err(format!(
            "Access denied while trying to {what} (try running as administrator)"
        ))
    } else {
        Err(format!(
            "Could not {what} (NTSTATUS 0x{:08X})",
            status.0 as u32
        ))
    }
}

// ───────────────────────────── Processes ─────────────────────────────

pub fn suspend(pid: u32) -> Result<(), String> {
    let h = open_process(pid, PROCESS_SUSPEND_RESUME, "suspend the process")?;
    // SAFETY: `h` is a valid handle with PROCESS_SUSPEND_RESUME access.
    nt_ok(unsafe { NtSuspendProcess(h.0) }, "suspend the process")
}

pub fn resume(pid: u32) -> Result<(), String> {
    let h = open_process(pid, PROCESS_SUSPEND_RESUME, "resume the process")?;
    // SAFETY: `h` is a valid handle with PROCESS_SUSPEND_RESUME access.
    nt_ok(unsafe { NtResumeProcess(h.0) }, "resume the process")
}

pub fn set_priority(pid: u32, p: Priority) -> Result<(), String> {
    let class = match p {
        Priority::Realtime => REALTIME_PRIORITY_CLASS,
        Priority::High => HIGH_PRIORITY_CLASS,
        Priority::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
        Priority::Normal => NORMAL_PRIORITY_CLASS,
        Priority::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
        Priority::Idle => IDLE_PRIORITY_CLASS,
    };
    let h = open_process(pid, PROCESS_SET_INFORMATION, "change the priority")?;
    // SAFETY: valid handle with PROCESS_SET_INFORMATION access.
    unsafe { SetPriorityClass(h.0, class) }.map_err(|e| friendly(e, "change the priority"))
}

pub fn get_priority(pid: u32) -> Result<Priority, String> {
    let h = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION, "read the priority")?;
    // SAFETY: valid handle with query access.
    let c = unsafe { GetPriorityClass(h.0) };
    Ok(match c {
        x if x == REALTIME_PRIORITY_CLASS.0 => Priority::Realtime,
        x if x == HIGH_PRIORITY_CLASS.0 => Priority::High,
        x if x == ABOVE_NORMAL_PRIORITY_CLASS.0 => Priority::AboveNormal,
        x if x == BELOW_NORMAL_PRIORITY_CLASS.0 => Priority::BelowNormal,
        x if x == IDLE_PRIORITY_CLASS.0 => Priority::Idle,
        0 => return Err("Could not read the priority".into()),
        _ => Priority::Normal,
    })
}

/// Returns (process mask, system mask).
pub fn get_affinity(pid: u32) -> Result<(u64, u64), String> {
    let h = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION, "read the affinity")?;
    let (mut pm, mut sm) = (0usize, 0usize);
    // SAFETY: valid handle; both out-pointers point to live locals.
    unsafe { GetProcessAffinityMask(h.0, &mut pm, &mut sm) }
        .map_err(|e| friendly(e, "read the affinity"))?;
    Ok((pm as u64, sm as u64))
}

pub fn set_affinity(pid: u32, mask: u64) -> Result<(), String> {
    if mask == 0 {
        return Err("Select at least one CPU".into());
    }
    let h = open_process(pid, PROCESS_SET_INFORMATION, "change the affinity")?;
    // SAFETY: valid handle with PROCESS_SET_INFORMATION access.
    unsafe { SetProcessAffinityMask(h.0, mask as usize) }
        .map_err(|e| friendly(e, "change the affinity"))
}

/// Write a full memory dump of the process to `path`.
pub fn write_dump(pid: u32, path: &Path) -> Result<(), String> {
    let h = open_process(
        pid,
        PROCESS_QUERY_INFORMATION | PROCESS_VM_READ | PROCESS_DUP_HANDLE,
        "create a dump",
    )?;
    let file = std::fs::File::create(path)
        .map_err(|e| format!("Could not create {}: {e}", path.display()))?;
    let hfile = HANDLE(file.as_raw_handle());
    let kind = MiniDumpWithFullMemory
        | MiniDumpWithHandleData
        | MiniDumpWithThreadInfo
        | MiniDumpWithUnloadedModules;
    // SAFETY: process and file handles are valid for the duration of the call; optional params are None.
    let r = unsafe { MiniDumpWriteDump(h.0, pid, hfile, kind, None, None, None) };
    drop(file);
    r.map_err(|e| {
        let _ = std::fs::remove_file(path);
        friendly(e, "create a dump")
    })
}

// ───────────────────────────── Threads ─────────────────────────────

fn filetime_ms(ft: FILETIME) -> u64 {
    (((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64) / 10_000
}

pub fn threads(pid: u32) -> Result<Vec<ThreadInfo>, String> {
    // SAFETY: snapshot handle is owned by `Owned`.
    let snap = Owned(
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }
            .map_err(|e| friendly(e, "list threads"))?,
    );
    let mut te = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut out = Vec::new();
    // SAFETY: `te.dwSize` is initialised as the API requires; `te` outlives the calls.
    let mut ok = unsafe { Thread32First(snap.0, &mut te) }.is_ok();
    while ok {
        if te.th32OwnerProcessID == pid {
            let (mut cpu_ms, mut name) = (0, String::new());
            if let Ok(th) = open_thread(
                te.th32ThreadID,
                THREAD_QUERY_LIMITED_INFORMATION,
                "query thread",
            ) {
                let (mut c, mut x, mut k, mut u) = (
                    FILETIME::default(),
                    FILETIME::default(),
                    FILETIME::default(),
                    FILETIME::default(),
                );
                // SAFETY: valid thread handle; out-params point to live locals.
                if unsafe { GetThreadTimes(th.0, &mut c, &mut x, &mut k, &mut u) }.is_ok() {
                    cpu_ms = filetime_ms(k) + filetime_ms(u);
                }
                // SAFETY: on success the returned buffer is owned by us and freed with LocalFree.
                if let Ok(p) = unsafe { GetThreadDescription(th.0) } {
                    if !p.is_null() {
                        name = unsafe { p.to_string() }.unwrap_or_default();
                        unsafe {
                            let _ = LocalFree(Some(HLOCAL(p.0 as *mut _)));
                        }
                    }
                }
            }
            out.push(ThreadInfo {
                tid: te.th32ThreadID,
                base_priority: te.tpBasePri,
                cpu_ms,
                name,
            });
        }
        // SAFETY: same as Thread32First.
        ok = unsafe { Thread32Next(snap.0, &mut te) }.is_ok();
    }
    Ok(out)
}

pub fn thread_action(tid: u32, action: ThreadAction) -> Result<(), String> {
    match action {
        ThreadAction::Suspend | ThreadAction::Resume => {
            let th = open_thread(tid, THREAD_SUSPEND_RESUME, "change the thread")?;
            // SAFETY: valid handle with THREAD_SUSPEND_RESUME access.
            let r = unsafe {
                if action == ThreadAction::Suspend {
                    SuspendThread(th.0)
                } else {
                    ResumeThread(th.0)
                }
            };
            if r == u32::MAX {
                Err("Could not change the thread state".into())
            } else {
                Ok(())
            }
        }
        ThreadAction::Kill => {
            let th = open_thread(tid, THREAD_TERMINATE, "end the thread")?;
            // SAFETY: valid handle with THREAD_TERMINATE access. Killing a thread can
            // destabilise the target process; the UI asks for confirmation first.
            unsafe { TerminateThread(th.0, 1) }.map_err(|e| friendly(e, "end the thread"))
        }
    }
}

// ───────────────────────────── Windows ─────────────────────────────

fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    // SAFETY: buffer is valid and its length is passed implicitly by the slice.
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: `pid` is a live local.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// The top-level window under the mouse pointer.
pub fn window_at_cursor() -> Option<WindowInfo> {
    let mut pt = POINT::default();
    // SAFETY: `pt` is a live local.
    unsafe { GetCursorPos(&mut pt) }.ok()?;
    // SAFETY: plain value arguments.
    let hwnd = unsafe { GetAncestor(WindowFromPoint(pt), GA_ROOT) };
    if hwnd.is_invalid() {
        return None;
    }
    Some(WindowInfo {
        pid: window_pid(hwnd),
        title: window_title(hwnd),
    })
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the &mut Vec passed by `top_windows`, alive for the whole EnumWindows call.
    let list = unsafe { &mut *(lparam.0 as *mut Vec<WindowInfo>) };
    // SAFETY: plain handle argument.
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return true.into();
    }
    let mut cloaked = 0u32;
    // SAFETY: out-buffer is a live u32 of the declared size.
    let _ = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            std::mem::size_of::<u32>() as u32,
        )
    };
    let title = window_title(hwnd);
    if cloaked == 0 && !title.trim().is_empty() {
        list.push(WindowInfo {
            pid: window_pid(hwnd),
            title,
        });
    }
    true.into()
}

/// Visible top-level windows with titles (like the taskbar shows).
pub fn top_windows() -> Vec<WindowInfo> {
    let mut list: Vec<WindowInfo> = Vec::new();
    // SAFETY: the callback only dereferences lparam as the Vec above, which outlives the call.
    let _ = unsafe { EnumWindows(Some(enum_cb), LPARAM(&mut list as *mut _ as isize)) };
    list
}

// ───────────────────────────── File locks ─────────────────────────────

struct RmSession(u32);
impl Drop for RmSession {
    fn drop(&mut self) {
        // SAFETY: session handle came from RmStartSession.
        unsafe {
            let _ = RmEndSession(self.0);
        }
    }
}

/// Processes holding any of `paths` open (folders are expanded to their files).
pub fn file_lockers(paths: &[PathBuf]) -> Result<Vec<Locker>, String> {
    let files = super::expand_paths(paths, 5000);
    if files.is_empty() {
        return Err("Nothing to check".into());
    }
    let mut session = 0u32;
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    // SAFETY: out-params are live; the key buffer has the documented size.
    let rc = unsafe { RmStartSession(&mut session, None, PWSTR(key.as_mut_ptr())) };
    if rc != ERROR_SUCCESS {
        return Err(format!("Restart Manager error {}", rc.0));
    }
    let session = RmSession(session);
    let wide: Vec<Vec<u16>> = files
        .iter()
        .map(|p| p.as_os_str().encode_wide_null())
        .collect();
    let ptrs: Vec<PCWSTR> = wide.iter().map(|w| PCWSTR(w.as_ptr())).collect();
    // SAFETY: every PCWSTR points into `wide`, which outlives the call.
    let rc = unsafe { RmRegisterResources(session.0, Some(&ptrs), None, None) };
    if rc != ERROR_SUCCESS {
        return Err(format!("Restart Manager error {}", rc.0));
    }
    let mut infos: Vec<RM_PROCESS_INFO> = Vec::new();
    for _ in 0..4 {
        let (mut needed, mut count, mut reasons) = (0u32, infos.len() as u32, 0u32);
        let ptr = if infos.is_empty() {
            None
        } else {
            Some(infos.as_mut_ptr())
        };
        // SAFETY: `count` matches the capacity of `infos`; all out-params are live.
        let rc = unsafe { RmGetList(session.0, &mut needed, &mut count, ptr, &mut reasons) };
        if rc == ERROR_SUCCESS {
            infos.truncate(count as usize);
            break;
        } else if rc == ERROR_MORE_DATA {
            infos = vec![RM_PROCESS_INFO::default(); needed as usize + 4];
        } else {
            return Err(format!("Restart Manager error {}", rc.0));
        }
    }
    let s = |w: &[u16]| {
        String::from_utf16_lossy(&w[..w.iter().position(|&c| c == 0).unwrap_or(w.len())])
    };
    let mut out: Vec<Locker> = infos
        .iter()
        .map(|i| Locker {
            pid: i.Process.dwProcessId,
            app: s(&i.strAppName),
            service: s(&i.strServiceShortName),
        })
        .collect();
    out.sort_by_key(|l| l.pid);
    out.dedup_by_key(|l| l.pid);
    Ok(out)
}

trait EncodeWideNull {
    fn encode_wide_null(&self) -> Vec<u16>;
}
impl EncodeWideNull for std::ffi::OsStr {
    fn encode_wide_null(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(std::iter::once(0)).collect()
    }
}

pub fn net_status_hint() -> &'static str {
    "Per-app network speed uses Windows event tracing, which needs administrator rights."
}

/// Press Win+Ctrl+Shift+B, Windows' built-in shortcut that restarts the
/// graphics driver (the screen blinks; apps keep running).
pub fn reset_graphics_driver() -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY, VK_B, VK_LCONTROL, VK_LSHIFT, VK_LWIN,
    };
    let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let keys = [VK_LWIN, VK_LCONTROL, VK_LSHIFT, VK_B];
    let mut inputs: Vec<INPUT> = keys.iter().map(|&k| key(k, false)).collect();
    inputs.extend(keys.iter().rev().map(|&k| key(k, true)));
    // SAFETY: `inputs` is a live slice of fully initialised INPUT structs and
    // the size argument is the size of one element, as SendInput requires.
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err("Windows blocked the key press (try again with TaskForge in front)".into())
    }
}
