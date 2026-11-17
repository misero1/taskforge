//! Windows shell, services and Task Scheduler through the official APIs.
//!
//! These replace earlier PowerShell scripts: calling Windows directly is
//! faster, doesn't spawn hidden script processes, and doesn't look like
//! malware to antivirus heuristics.

use std::path::{Path, PathBuf};

use windows::core::{Interface, BSTR, HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_CANCELLED, ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Services::{
    CloseServiceHandle, EnumServicesStatusExW, OpenSCManagerW, OpenServiceW, QueryServiceConfig2W,
    QueryServiceConfigW, ENUM_SERVICE_STATUS_PROCESSW, QUERY_SERVICE_CONFIGW, SC_ENUM_PROCESS_INFO,
    SC_HANDLE, SC_MANAGER_CONNECT, SC_MANAGER_ENUMERATE_SERVICE,
    SERVICE_CONFIG_DELAYED_AUTO_START_INFO, SERVICE_CONFIG_DESCRIPTION,
    SERVICE_DELAYED_AUTO_START_INFO, SERVICE_DESCRIPTIONW, SERVICE_QUERY_CONFIG, SERVICE_STATE_ALL,
    SERVICE_WIN32,
};
use windows::Win32::System::TaskScheduler::{
    IExecAction, IRegisteredTask, ITaskFolder, ITaskService, TaskScheduler, TASK_ACTION_EXEC,
    TASK_ENUM_HIDDEN, TASK_TRIGGER_BOOT, TASK_TRIGGER_LOGON,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Programs, IShellLinkW, SHGetKnownFolderPath, ShellExecuteExW,
    ShellLink, KF_FLAG_DEFAULT, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MB_ICONINFORMATION, MB_OK, SW_HIDE, SW_SHOWNORMAL,
};

use crate::platform::{SchedTask, Service};

/// Initialise COM for the current thread for as long as the guard lives.
struct Com(bool);

impl Com {
    fn init() -> Com {
        // SAFETY: plain COM initialisation; balanced by Drop only when it succeeded.
        // If the thread already uses another apartment, COM is usable anyway.
        let ok = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        Com(ok)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx above on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

fn wide(s: &str) -> HSTRING {
    HSTRING::from(s)
}

/// Start a program or open a file through the Windows shell. `admin` asks
/// for elevation with the normal UAC prompt.
pub fn shell_execute(file: &str, params: &str, admin: bool, visible: bool) -> Result<(), String> {
    let file_w = wide(file);
    let params_w = wide(params);
    let verb_w = wide(if admin { "runas" } else { "open" });
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: PCWSTR(verb_w.as_ptr()),
        lpFile: PCWSTR(file_w.as_ptr()),
        lpParameters: if params.is_empty() {
            PCWSTR::null()
        } else {
            PCWSTR(params_w.as_ptr())
        },
        nShow: if visible { SW_SHOWNORMAL.0 } else { SW_HIDE.0 },
        ..Default::default()
    };
    // SAFETY: `info` is fully initialised and the strings it points to outlive the call.
    match unsafe { ShellExecuteExW(&mut info) } {
        Ok(()) => Ok(()),
        Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => {
            Err("Cancelled: administrator permission was not given".into())
        }
        Err(e) => Err(format!("Could not start {file}: {}", e.message())),
    }
}

pub fn message_box(title: &str, text: &str) {
    let (t, x) = (wide(title), wide(text));
    // SAFETY: both strings are valid, null-terminated and live for the call.
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(x.as_ptr()),
            PCWSTR(t.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

/// The user's Start-menu Programs folder.
pub fn programs_folder() -> Option<PathBuf> {
    known_folder(&FOLDERID_Programs)
}

pub fn desktop_folder() -> Option<PathBuf> {
    known_folder(&FOLDERID_Desktop)
}

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    // SAFETY: the returned buffer is owned by us and freed with CoTaskMemFree.
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// Create (or overwrite) a .lnk shortcut.
pub fn create_shortcut(
    lnk: &Path,
    target: &Path,
    workdir: &Path,
    description: &str,
) -> Result<(), String> {
    let _com = Com::init();
    let target_w = wide(&target.display().to_string());
    let dir_w = wide(&workdir.display().to_string());
    let desc_w = wide(description);
    let lnk_w = wide(&lnk.display().to_string());
    // SAFETY: standard IShellLink usage; every string outlives the calls that use it.
    unsafe {
        let link: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;
        link.SetPath(PCWSTR(target_w.as_ptr()))
            .map_err(|e| e.message())?;
        link.SetWorkingDirectory(PCWSTR(dir_w.as_ptr()))
            .map_err(|e| e.message())?;
        link.SetIconLocation(PCWSTR(target_w.as_ptr()), 0)
            .map_err(|e| e.message())?;
        link.SetDescription(PCWSTR(desc_w.as_ptr()))
            .map_err(|e| e.message())?;
        let file: IPersistFile = link.cast().map_err(|e| e.message())?;
        file.Save(PCWSTR(lnk_w.as_ptr()), true)
            .map_err(|e| e.message())?;
    }
    Ok(())
}

// ───────────────────────────── Services ─────────────────────────────

struct Scm(SC_HANDLE);
impl Drop for Scm {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenSCManagerW/OpenServiceW and is closed once.
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

fn state_name(state: u32) -> &'static str {
    match state {
        1 => "Stopped",
        2 => "Start Pending",
        3 => "Stop Pending",
        4 => "Running",
        5 => "Continue Pending",
        6 => "Pause Pending",
        7 => "Paused",
        _ => "Unknown",
    }
}

fn start_name(start: u32, delayed: bool) -> &'static str {
    match start {
        0 => "Boot",
        1 => "System",
        2 if delayed => "Automatic (delayed)",
        2 => "Automatic",
        3 => "Manual",
        4 => "Disabled",
        _ => "Unknown",
    }
}

/// Read a variable-size service structure into an 8-byte-aligned buffer.
fn query_buf(
    mut f: impl FnMut(Option<&mut [u8]>, &mut u32) -> windows::core::Result<()>,
) -> Option<Vec<u64>> {
    let mut needed = 0u32;
    if let Err(e) = f(None, &mut needed) {
        if e.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult() {
            return None;
        }
    }
    let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
    // SAFETY: viewing the u64 buffer as bytes of the same length.
    let bytes =
        unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, buf.len() * 8) };
    f(Some(bytes), &mut needed).ok()?;
    Some(buf)
}

pub fn list_services() -> Result<Vec<Service>, String> {
    // SAFETY: SCM calls with buffers sized from the API's own size reports; all
    // pointers read below point into those buffers, which outlive the reads.
    unsafe {
        let scm = Scm(OpenSCManagerW(
            PCWSTR::null(),
            PCWSTR::null(),
            SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE,
        )
        .map_err(|e| format!("Could not open the service list: {}", e.message()))?);
        let mut buf: Vec<u64> = vec![0; 32 * 1024];
        let mut resume = 0u32;
        let mut out = Vec::new();
        loop {
            let mut needed = 0u32;
            let mut count = 0u32;
            let bytes = std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, buf.len() * 8);
            let r = EnumServicesStatusExW(
                scm.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                Some(bytes),
                &mut needed,
                &mut count,
                Some(&mut resume),
                PCWSTR::null(),
            );
            let more = match &r {
                Ok(()) => false,
                Err(e) if e.code() == ERROR_MORE_DATA.to_hresult() => true,
                Err(e) => return Err(format!("Could not list services: {}", e.message())),
            };
            let items = std::slice::from_raw_parts(
                buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW,
                count as usize,
            );
            for it in items {
                let name = it.lpServiceName.to_string().unwrap_or_default();
                let display = it.lpDisplayName.to_string().unwrap_or_default();
                let st = &it.ServiceStatusProcess;
                let (mut start, mut path, mut desc, mut delayed) =
                    (u32::MAX, String::new(), String::new(), false);
                if let Ok(h) =
                    OpenServiceW(scm.0, PCWSTR(wide(&name).as_ptr()), SERVICE_QUERY_CONFIG)
                {
                    let h = Scm(h);
                    if let Some(b) = query_buf(|b, n| {
                        let size = b.as_ref().map_or(0, |b| b.len() as u32);
                        QueryServiceConfigW(
                            h.0,
                            b.map(|b| b.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW),
                            size,
                            n,
                        )
                    }) {
                        let c = &*(b.as_ptr() as *const QUERY_SERVICE_CONFIGW);
                        start = c.dwStartType.0;
                        path = c.lpBinaryPathName.to_string().unwrap_or_default();
                    }
                    if let Some(b) = query_buf(|b, n| {
                        QueryServiceConfig2W(h.0, SERVICE_CONFIG_DESCRIPTION, b, n)
                    }) {
                        let d = &*(b.as_ptr() as *const SERVICE_DESCRIPTIONW);
                        if !d.lpDescription.is_null() {
                            desc = d.lpDescription.to_string().unwrap_or_default();
                        }
                    }
                    if start == 2 {
                        if let Some(b) = query_buf(|b, n| {
                            QueryServiceConfig2W(h.0, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, b, n)
                        }) {
                            delayed = (*(b.as_ptr() as *const SERVICE_DELAYED_AUTO_START_INFO))
                                .fDelayedAutostart
                                .as_bool();
                        }
                    }
                }
                out.push(Service {
                    name,
                    display,
                    state: state_name(st.dwCurrentState.0).into(),
                    start_mode: start_name(start, delayed).into(),
                    pid: st.dwProcessId,
                    description: desc,
                    path,
                });
            }
            if !more {
                break;
            }
            if needed as usize > buf.len() * 8 {
                buf = vec![0; (needed as usize).div_ceil(8)];
            }
        }
        Ok(out)
    }
}

// ───────────────────────────── Scheduled tasks ─────────────────────────────

/// Scheduled tasks that run at sign-in or at boot.
pub fn list_logon_boot_tasks() -> Result<Vec<SchedTask>, String> {
    let _com = Com::init();
    // SAFETY: standard Task Scheduler 2.0 COM usage; interfaces are released on drop.
    unsafe {
        let svc: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("Task Scheduler unavailable: {}", e.message()))?;
        let empty = VARIANT::default();
        svc.Connect(&empty, &empty, &empty, &empty)
            .map_err(|e| format!("Could not connect to Task Scheduler: {}", e.message()))?;
        let root = svc.GetFolder(&BSTR::from("\\")).map_err(|e| e.message())?;
        let mut out = Vec::new();
        walk_folder(&root, &mut out, 0);
        Ok(out)
    }
}

unsafe fn walk_folder(folder: &ITaskFolder, out: &mut Vec<SchedTask>, depth: u32) {
    if depth > 16 {
        return;
    }
    let folder_path = folder.Path().map(|b| b.to_string()).unwrap_or_default();
    let folder_path = if folder_path.ends_with('\\') {
        folder_path
    } else {
        format!("{folder_path}\\")
    };
    if let Ok(tasks) = folder.GetTasks(TASK_ENUM_HIDDEN.0) {
        let n = tasks.Count().unwrap_or(0);
        for i in 1..=n {
            if let Ok(t) = tasks.get_Item(&VARIANT::from(i)) {
                if let Some(task) = read_task(&t, &folder_path) {
                    out.push(task);
                }
            }
        }
    }
    if let Ok(subs) = folder.GetFolders(0) {
        let n = subs.Count().unwrap_or(0);
        for i in 1..=n {
            if let Ok(f) = subs.get_Item(&VARIANT::from(i)) {
                walk_folder(&f, out, depth + 1);
            }
        }
    }
}

unsafe fn read_task(t: &IRegisteredTask, folder_path: &str) -> Option<SchedTask> {
    let def = t.Definition().ok()?;
    let triggers = def.Triggers().ok()?;
    let (mut logon, mut boot) = (false, false);
    let mut count = 0;
    let _ = triggers.Count(&mut count);
    for i in 1..=count {
        if let Ok(tr) = triggers.get_Item(i) {
            let mut ty = Default::default();
            if tr.Type(&mut ty).is_ok() {
                logon |= ty == TASK_TRIGGER_LOGON;
                boot |= ty == TASK_TRIGGER_BOOT;
            }
        }
    }
    if !logon && !boot {
        return None;
    }
    let mut actions = Vec::new();
    if let Ok(acts) = def.Actions() {
        let mut n = 0;
        let _ = acts.Count(&mut n);
        for i in 1..=n {
            let Ok(a) = acts.get_Item(i) else { continue };
            let mut ty = Default::default();
            if a.Type(&mut ty).is_ok() && ty == TASK_ACTION_EXEC {
                if let Ok(e) = a.cast::<IExecAction>() {
                    let mut p = BSTR::new();
                    let mut args = BSTR::new();
                    let _ = e.Path(&mut p);
                    let _ = e.Arguments(&mut args);
                    actions.push(format!("{p} {args}").trim().to_string());
                }
            }
        }
    }
    let mut author = BSTR::new();
    if let Ok(info) = def.RegistrationInfo() {
        let _ = info.Author(&mut author);
    }
    let state = match t.State().map(|s| s.0).unwrap_or(0) {
        1 => "Disabled",
        2 => "Queued",
        3 => "Ready",
        4 => "Running",
        _ => "Unknown",
    };
    let trigger = match (logon, boot) {
        (true, true) => "Sign-in, Boot",
        (true, false) => "Sign-in",
        _ => "Boot",
    };
    Some(SchedTask {
        name: t.Name().map(|b| b.to_string()).unwrap_or_default(),
        path: folder_path.to_string(),
        state: state.into(),
        trigger: trigger.into(),
        action: actions.join("; "),
        author: author.to_string(),
    })
}
