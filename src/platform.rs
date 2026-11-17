//! Operating-system actions.
//!
//! Everything here drives the tools that ship with Windows (`taskkill`, `reg`,
//! `sc`, `netstat`, PowerShell) instead of raw Win32 FFI. That keeps the code
//! safe, easy to audit, and lets the parsers be unit-tested on any OS.
//! Where a sensible equivalent exists, non-Windows fallbacks are provided so the
//! app still runs for development.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

pub const IS_WINDOWS: bool = cfg!(windows);

/// `TASKFORGE_DEMO=1` feeds sample Windows output through the real parsers.
/// Used for screenshots and UI testing on machines without the Windows tools.
fn demo() -> bool {
    std::env::var_os("TASKFORGE_DEMO").is_some()
}

const DEMO_NETSTAT: &str = "\r\nActive Connections\r\n\r\n  Proto  Local Address          Foreign Address        State           PID\r\n  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1084\r\n  TCP    0.0.0.0:445            0.0.0.0:0              LISTENING       4\r\n  TCP    192.168.1.42:52114     140.82.112.25:443      ESTABLISHED     1\r\n  TCP    192.168.1.42:52120     142.250.72.110:443     ESTABLISHED     1\r\n  TCP    192.168.1.42:52188     20.42.65.92:443        ESTABLISHED     71\r\n  TCP    192.168.1.42:52190     52.96.111.34:443       TIME_WAIT       0\r\n  TCP    127.0.0.1:9229         127.0.0.1:52211        ESTABLISHED     71\r\n  TCP    192.168.1.42:52230     192.168.1.1:53         ESTABLISHED     98\r\n  UDP    0.0.0.0:5353           *:*                                    1\r\n  UDP    [::]:500               *:*                                    3920\r\n";

const DEMO_TASKS: &str = "\"Name\",\"Path\",\"State\",\"Trigger\",\"Action\",\"Author\"\r\n\"OneDrive Reporting Task\",\"\\\",\"Ready\",\"Sign-in\",\"%localappdata%\\Microsoft\\OneDrive\\OneDriveStandaloneUpdater.exe /reporting\",\"Microsoft\"\r\n\"GoogleUpdateTaskMachineCore\",\"\\\",\"Ready\",\"Sign-in, Boot\",\"C:\\Program Files (x86)\\Google\\Update\\GoogleUpdate.exe /c\",\"Google\"\r\n\"Adobe Acrobat Update Task\",\"\\\",\"Disabled\",\"Sign-in\",\"C:\\Program Files (x86)\\Common Files\\Adobe\\ARM\\1.0\\AdobeARM.exe\",\"Adobe\"\r\n\"NvTmRep_CrashReport1\",\"\\NVIDIA\\\",\"Ready\",\"Sign-in\",\"C:\\Program Files\\NVIDIA Corporation\\NvTmRep.exe --crash-report\",\"NVIDIA\"\r\n\"MicrosoftEdgeUpdateTaskMachineCore\",\"\\Microsoft\\Edge\\\",\"Ready\",\"Sign-in\",\"C:\\Program Files (x86)\\Microsoft\\EdgeUpdate\\MicrosoftEdgeUpdate.exe /c\",\"\"\r\n";

const DEMO_SERVICES: &str = "\"Name\",\"DisplayName\",\"State\",\"StartMode\",\"DelayedAutoStart\",\"ProcessId\",\"Description\",\"PathName\"\r\n\"AudioSrv\",\"Windows Audio\",\"Running\",\"Auto\",\"False\",\"1\",\"Manages audio for Windows-based programs.\",\"C:\\Windows\\System32\\svchost.exe -k LocalServiceNetworkRestricted -p\"\r\n\"BITS\",\"Background Intelligent Transfer Service\",\"Running\",\"Auto\",\"True\",\"71\",\"Transfers files in the background using idle network bandwidth.\",\"C:\\Windows\\System32\\svchost.exe -k netsvcs -p\"\r\n\"DiagTrack\",\"Connected User Experiences and Telemetry\",\"Stopped\",\"Disabled\",\"False\",\"0\",\"Enables collection of diagnostic data.\",\"\"\r\n\"Spooler\",\"Print Spooler\",\"Stopped\",\"Manual\",\"False\",\"0\",\"Spools print jobs and handles interaction with the printer.\",\"C:\\Windows\\System32\\spoolsv.exe\"\r\n\"SteamClient\",\"Steam Client Service\",\"Stopped\",\"Auto\",\"False\",\"0\",\"Provides Steam features.\",\"C:\\Program Files (x86)\\Common Files\\Steam\\steamservice.exe /RunAsService\"\r\n\"AdobeARMservice\",\"Adobe Acrobat Update Service\",\"Running\",\"Auto\",\"False\",\"98\",\"Keeps Adobe Acrobat up to date.\",\"C:\\Program Files (x86)\\Common Files\\Adobe\\ARM\\1.0\\armsvc.exe\"\r\n\"WSearch\",\"Windows Search\",\"Running\",\"Auto\",\"True\",\"98\",\"Provides content indexing and property caching.\",\"C:\\Windows\\system32\\SearchIndexer.exe /Embedding\"\r\n\"wuauserv\",\"Windows Update\",\"Running\",\"Manual\",\"False\",\"71\",\"Enables the detection, download and installation of updates.\",\"C:\\Windows\\system32\\svchost.exe -k netsvcs -p\"\r\n";

/// Build a `Command` that never flashes a console window on Windows.
pub fn command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

fn finish(out: std::io::Result<Output>) -> Result<String, String> {
    match out {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
            if o.status.success() {
                Ok(stdout)
            } else {
                let msg = [stderr.trim(), stdout.trim()]
                    .into_iter()
                    .find(|s| !s.is_empty())
                    .unwrap_or("command failed")
                    .to_string();
                Err(clean_error(&msg))
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Shorten verbose tool / PowerShell error text to its first meaningful line.
fn clean_error(msg: &str) -> String {
    let line = msg
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("At line") && !l.starts_with('+'))
        .unwrap_or(msg)
        .to_string();
    if line.len() > 220 {
        format!(
            "{}…",
            &line[..line
                .char_indices()
                .nth(220)
                .map(|c| c.0)
                .unwrap_or(line.len())]
        )
    } else {
        line
    }
}

pub fn run(program: &str, args: &[&str]) -> Result<String, String> {
    finish(command(program).args(args).output())
}

/// Run a short PowerShell command (only used for the on-demand signature
/// check). The command is passed as plain text: no encoding and no
/// execution-policy override, so it's fully visible to security software.
pub fn powershell(script: &str) -> Result<String, String> {
    let full = format!(
        "$ProgressPreference='SilentlyContinue';[Console]::OutputEncoding=[Text.Encoding]::UTF8;{script}"
    );
    let exe = if IS_WINDOWS { "powershell" } else { "pwsh" };
    finish(
        command(exe)
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(full)
            .output(),
    )
}

/// Quote a string as a PowerShell single-quoted literal.
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

// ───────────────────────────── Processes ─────────────────────────────

pub fn kill(pid: u32, tree: bool) -> Result<String, String> {
    if IS_WINDOWS {
        let p = pid.to_string();
        let mut args = vec!["/PID", p.as_str(), "/F"];
        if tree {
            args.push("/T");
        }
        run("taskkill", &args).map(|s| s.trim().to_string())
    } else {
        if tree {
            let _ = run("pkill", &["-KILL", "-P", &pid.to_string()]);
        }
        run("kill", &["-9", &pid.to_string()]).map(|_| format!("Killed {pid}"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Priority {
    Realtime,
    High,
    AboveNormal,
    Normal,
    BelowNormal,
    Idle,
}

impl Priority {
    pub const ALL: [Priority; 6] = [
        Priority::Realtime,
        Priority::High,
        Priority::AboveNormal,
        Priority::Normal,
        Priority::BelowNormal,
        Priority::Idle,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Priority::Realtime => "Realtime",
            Priority::High => "High",
            Priority::AboveNormal => "Above normal",
            Priority::Normal => "Normal",
            Priority::BelowNormal => "Below normal",
            Priority::Idle => "Low (Idle)",
        }
    }
    #[allow(dead_code)]
    fn nice(self) -> i32 {
        match self {
            Priority::Realtime => -20,
            Priority::High => -10,
            Priority::AboveNormal => -5,
            Priority::Normal => 0,
            Priority::BelowNormal => 5,
            Priority::Idle => 19,
        }
    }
}

pub fn set_priority(pid: u32, p: Priority) -> Result<String, String> {
    #[cfg(windows)]
    let r = crate::native::set_priority(pid, p);
    #[cfg(not(windows))]
    let r = run(
        "renice",
        &["-n", &p.nice().to_string(), "-p", &pid.to_string()],
    )
    .map(|_| ());
    r.map(|_| format!("Priority set to {}", p.label()))
}

pub fn get_priority(pid: u32) -> Result<String, String> {
    crate::native::get_priority(pid).map(|p| p.label().to_string())
}

pub fn get_affinity(pid: u32) -> Result<u64, String> {
    crate::native::get_affinity(pid).map(|m| m.0)
}

pub fn set_affinity(pid: u32, mask: u64) -> Result<String, String> {
    crate::native::set_affinity(pid, mask)
        .map(|_| format!("Affinity set to {} CPU(s)", mask.count_ones()))
}

/// Open Explorer with the file selected.
pub fn open_location(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("Path is not available (try running as administrator)".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Explorer has its own argument parser; pass the switch verbatim.
        Command::new("explorer")
            .raw_arg(format!("/select,\"{path}\""))
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let dir = Path::new(path).parent().unwrap_or(Path::new("/"));
        Command::new("xdg-open")
            .arg(dir)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

pub fn open_url(url: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = command("cmd")
            .raw_arg(format!("/C start \"\" \"{url}\""))
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("xdg-open").arg(url).spawn();
    }
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Launch a command line, optionally elevated.
pub fn run_task(cmdline: &str, admin: bool) -> Result<String, String> {
    let cmdline = cmdline.trim();
    if cmdline.is_empty() {
        return Err("Nothing to run".into());
    }
    if IS_WINDOWS {
        crate::native::shell_execute("cmd.exe", &format!("/c start \"\" {cmdline}"), admin, false)
            .map(|_| format!("Started: {cmdline}"))
    } else {
        Command::new("sh")
            .args(["-c", &format!("{cmdline} &")])
            .spawn()
            .map(|_| format!("Started: {cmdline}"))
            .map_err(|e| e.to_string())
    }
}

pub fn is_admin() -> bool {
    static ADMIN: OnceLock<bool> = OnceLock::new();
    *ADMIN.get_or_init(|| {
        if IS_WINDOWS {
            command("net")
                .arg("session")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        } else {
            run("id", &["-u"]).map(|s| s.trim() == "0").unwrap_or(false)
        }
    })
}

/// Relaunch this executable elevated. Returns Ok when the new instance started.
pub fn relaunch_as_admin() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if !IS_WINDOWS {
        return Err("Run with sudo to elevate on this platform".into());
    }
    crate::native::shell_execute(&exe.display().to_string(), "", true, true)
}

// ───────────────────────────── Install / uninstall ─────────────────────────────
//
// TaskForge is a single self-contained exe. "Installing" copies it to the
// per-user programs folder, adds shortcuts and registers it with Windows'
// installed-apps list (no administrator rights needed). `--uninstall` reverses it.

const UNINSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\TaskForge";
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

pub fn install_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Programs").join("TaskForge"))
}

pub fn installed_exe() -> Option<PathBuf> {
    install_dir().map(|d| d.join("TaskForge.exe"))
}

pub fn is_installed() -> bool {
    IS_WINDOWS && installed_exe().is_some_and(|p| p.exists())
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        std::fs::canonicalize(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .display()
            .to_string()
            .trim_start_matches(r"\\?\")
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// True when this process is the installed copy.
pub fn running_installed() -> bool {
    match (std::env::current_exe(), installed_exe()) {
        (Ok(cur), Some(inst)) => same_path(&cur, &inst),
        _ => false,
    }
}

pub fn install(desktop_shortcut: bool, start_at_login: bool) -> Result<PathBuf, String> {
    if !IS_WINDOWS {
        return Err("Installing is only available on Windows".into());
    }
    let src = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = install_dir().ok_or("LOCALAPPDATA is not set")?;
    let dst = dir.join("TaskForge.exe");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    if !same_path(&src, &dst) {
        std::fs::copy(&src, &dst).map_err(|e| {
            format!(
                "Could not copy to {} ({e}). Close any other running TaskForge and try again",
                dst.display()
            )
        })?;
    }
    let size_kb = std::fs::metadata(&dst).map(|m| m.len() / 1024).unwrap_or(0);
    let desc = "TaskForge - modern Task Manager";
    let programs =
        crate::native::programs_folder().ok_or("Could not find the Start menu folder")?;
    crate::native::create_shortcut(&programs.join("TaskForge.lnk"), &dst, &dir, desc)
        .map_err(|e| format!("Could not create the Start menu shortcut: {e}"))?;
    if desktop_shortcut {
        if let Some(desk) = crate::native::desktop_folder() {
            let _ = crate::native::create_shortcut(&desk.join("TaskForge.lnk"), &dst, &dir, desc);
        }
    }
    // Register with Settings > Apps > Installed apps.
    let exe = dst.display().to_string();
    let strings = [
        ("DisplayName", "TaskForge".to_string()),
        ("DisplayVersion", env!("CARGO_PKG_VERSION").to_string()),
        ("Publisher", "TaskForge".to_string()),
        ("DisplayIcon", format!("{exe},0")),
        ("InstallLocation", dir.display().to_string()),
        ("UninstallString", format!("\"{exe}\" --uninstall")),
        (
            "QuietUninstallString",
            format!("\"{exe}\" --uninstall --quiet"),
        ),
    ];
    for (name, value) in &strings {
        run(
            "reg",
            &[
                "add",
                UNINSTALL_KEY,
                "/v",
                name,
                "/t",
                "REG_SZ",
                "/d",
                value,
                "/f",
            ],
        )?;
    }
    for (name, value) in [
        ("NoModify", 1u64),
        ("NoRepair", 1),
        ("EstimatedSize", size_kb),
    ] {
        run(
            "reg",
            &[
                "add",
                UNINSTALL_KEY,
                "/v",
                name,
                "/t",
                "REG_DWORD",
                "/d",
                &value.to_string(),
                "/f",
            ],
        )?;
    }
    set_start_at_login(start_at_login)?;
    Ok(dst)
}

/// Remove shortcuts, registry entries and (after this process exits) the install folder.
pub fn uninstall() -> Result<String, String> {
    if !IS_WINDOWS {
        return Err("Uninstalling is only available on Windows".into());
    }
    let dir = install_dir().ok_or("LOCALAPPDATA is not set")?;
    let _ = set_start_at_login(false);
    for folder in [
        crate::native::programs_folder(),
        crate::native::desktop_folder(),
    ]
    .into_iter()
    .flatten()
    {
        let lnk = folder.join("TaskForge.lnk");
        if lnk.exists() {
            let _ = std::fs::remove_file(&lnk);
        }
    }
    let _ = run("reg", &["delete", UNINSTALL_KEY, "/f"]);
    let _ = dir;
    Ok("TaskForge was uninstalled".into())
}

/// Delete the install folder a few seconds after this process exits
/// (the running exe can't delete itself while it's open).
pub fn schedule_install_dir_removal() {
    let Some(dir) = install_dir() else { return };
    let cleanup = format!(
        "ping -n 4 127.0.0.1 >nul & rmdir /s /q \"{}\"",
        dir.display()
    );
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = command("cmd").raw_arg(format!("/C {cleanup}")).spawn();
    }
    #[cfg(not(windows))]
    let _ = cleanup;
}

pub fn start_at_login_enabled() -> bool {
    IS_WINDOWS
        && reg_values(RUN_KEY)
            .iter()
            .any(|v| v.name.eq_ignore_ascii_case("TaskForge"))
}

pub fn set_start_at_login(on: bool) -> Result<(), String> {
    if !IS_WINDOWS {
        return Err("Only available on Windows".into());
    }
    if on {
        let exe = installed_exe()
            .filter(|p| p.exists())
            .or_else(|| std::env::current_exe().ok())
            .ok_or("Could not find TaskForge.exe")?;
        let cmd = format!("\"{}\" --mini", exe.display());
        run(
            "reg",
            &[
                "add",
                RUN_KEY,
                "/v",
                "TaskForge",
                "/t",
                "REG_SZ",
                "/d",
                &cmd,
                "/f",
            ],
        )
        .map(|_| ())
    } else if start_at_login_enabled() {
        run("reg", &["delete", RUN_KEY, "/v", "TaskForge", "/f"]).map(|_| ())
    } else {
        Ok(())
    }
}

/// Native message box (used by `--uninstall`, which runs without a window).
pub fn message_box(title: &str, text: &str) {
    if IS_WINDOWS {
        crate::native::message_box(title, text);
    } else {
        eprintln!("{title}: {text}");
    }
}

/// Start a program detached (used to launch the freshly installed copy).
pub fn launch(exe: &Path) -> Result<(), String> {
    command(&exe.display().to_string())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// ───────────────────────────── Restart, signatures, hashes ─────────────────────────────

/// End a process and start it again with the same program, arguments and folder.
pub fn restart_process(pid: u32, exe: &str, args: &[String], cwd: &str) -> Result<String, String> {
    let program = if !exe.is_empty() {
        exe.to_string()
    } else {
        args.first().cloned().unwrap_or_default()
    };
    if program.is_empty() {
        return Err(
            "Can't restart: the program path is unknown (try running as administrator)".into(),
        );
    }
    kill(pid, false)?;
    // Give single-instance apps a moment to release their lock.
    std::thread::sleep(std::time::Duration::from_millis(700));
    let mut c = Command::new(&program);
    c.args(args.iter().skip(1));
    if !cwd.is_empty() && Path::new(cwd).is_dir() {
        c.current_dir(cwd);
    }
    c.spawn()
        .map(|ch| format!("Restarted (new PID {})", ch.id()))
        .map_err(|e| format!("Ended, but could not start again: {e}"))
}

#[derive(Clone, Debug, PartialEq)]
pub struct SigInfo {
    /// "Verified", "Not signed", "Invalid", …
    pub status: String,
    pub signer: String,
}

impl SigInfo {
    pub fn verified(&self) -> bool {
        self.status == "Verified"
    }
}

/// Parse `path|Status|Signer` lines produced by `verify_signatures`.
/// Keys are lower-cased paths.
pub fn parse_sig_lines(text: &str) -> std::collections::HashMap<String, SigInfo> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.trim_end_matches('\r').splitn(3, '|');
            let path = it.next()?.to_string();
            let status = it.next()?.trim();
            let signer = it.next().unwrap_or("").trim().to_string();
            let status = match status {
                "Valid" => "Verified",
                "NotSigned" => "Not signed",
                "HashMismatch" => "Tampered (hash mismatch)",
                "NotTrusted" => "Not trusted",
                "Error" | "UnknownError" => "Unknown",
                other => other,
            }
            .to_string();
            (!path.is_empty()).then_some((path.to_lowercase(), SigInfo { status, signer }))
        })
        .collect()
}

/// Check Authenticode signatures for many files in one PowerShell call.
pub fn verify_signatures(
    paths: &[String],
) -> Result<std::collections::HashMap<String, SigInfo>, String> {
    if !IS_WINDOWS {
        return Err("Signature verification is only available on Windows".into());
    }
    if paths.is_empty() {
        return Ok(Default::default());
    }
    let list = paths
        .iter()
        .map(|p| ps_quote(p))
        .collect::<Vec<_>>()
        .join(",");
    let out = powershell(&format!(
        "$ErrorActionPreference='Continue'; foreach ($f in @({list})) {{ try {{ \
         $s = Get-AuthenticodeSignature -LiteralPath $f; $n = ''; \
         if ($s.SignerCertificate) {{ $n = $s.SignerCertificate.GetNameInfo('SimpleName', $false) }}; \
         \"$f|$($s.Status)|$n\" }} catch {{ \"$f|Error|\" }} }}"
    ))?;
    Ok(parse_sig_lines(&out))
}

/// SHA-256 of a file as lowercase hex (used for VirusTotal look-ups).
pub fn sha256_file(path: &str) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("Could not read {path}: {e}"))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub fn virustotal_url(sha256: &str) -> String {
    format!("https://www.virustotal.com/gui/file/{sha256}")
}

/// Restart Windows Explorer (taskbar / desktop) — a common fix-it action.
pub fn restart_explorer() -> Result<String, String> {
    if !IS_WINDOWS {
        return Err("Only available on Windows".into());
    }
    let _ = run("taskkill", &["/F", "/IM", "explorer.exe"]);
    std::thread::sleep(std::time::Duration::from_millis(800));
    Command::new("explorer.exe")
        .spawn()
        .map(|_| "Windows Explorer restarted".to_string())
        .map_err(|e| e.to_string())
}

/// Open a built-in Windows tool (resmon, eventvwr, services.msc, …).
pub fn launch_tool(cmd: &str) -> Result<String, String> {
    if !IS_WINDOWS {
        return Err("Only available on Windows".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command("cmd")
            .raw_arg(format!("/C start \"\" {cmd}"))
            .spawn()
            .map(|_| format!("Opened {cmd}"))
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    Ok(cmd.to_string())
}

// ───────────────────────────── Registry helpers ─────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct RegValue {
    pub name: String,
    pub kind: String,
    pub data: String,
}

/// Parse the output of `reg query <key>` (values only, sub-keys ignored).
pub fn parse_reg_query(text: &str) -> Vec<RegValue> {
    let mut out = Vec::new();
    for line in text.lines() {
        if !line.starts_with("    ") {
            continue;
        }
        let Some(idx) = line.find("    REG_") else {
            continue;
        };
        let name = line[..idx].trim().to_string();
        let rest = &line[idx + 4..];
        let (kind, data) = match rest.find("    ") {
            Some(i) => (rest[..i].trim(), rest[i..].trim()),
            None => (rest.trim(), ""),
        };
        out.push(RegValue {
            name: if name == "(Default)" {
                String::new()
            } else {
                name
            },
            kind: kind.to_string(),
            data: data.to_string(),
        });
    }
    out
}

fn reg_values(key: &str) -> Vec<RegValue> {
    run("reg", &["query", key])
        .map(|t| parse_reg_query(&t))
        .unwrap_or_default()
}

// ───────────────────────────── Startup apps ─────────────────────────────

#[derive(Clone, Debug)]
pub struct StartupItem {
    pub name: String,
    pub command: String,
    pub enabled: bool,
    /// Human-readable origin, e.g. "Registry (current user)".
    pub source: String,
    /// Executable path extracted from `command`, if any.
    pub exe: String,
    /// Registry key holding the Run value, if registry-based.
    pub run_key: Option<String>,
    /// File path, if Startup-folder-based.
    pub file: Option<PathBuf>,
    /// Key under which Windows stores the enabled/disabled flag.
    pub approved_key: String,
    pub approved_name: String,
    pub needs_admin: bool,
}

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN32: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

/// Windows stores startup state as REG_BINARY whose first byte is even when enabled.
pub fn approved_flag(data: &str) -> bool {
    let first = data.get(0..2).and_then(|h| u8::from_str_radix(h, 16).ok());
    first.map(|b| b % 2 == 0).unwrap_or(true)
}

/// Pull the executable path out of a command line.
pub fn exe_from_command(cmd: &str) -> String {
    let c = cmd.trim();
    if let Some(rest) = c.strip_prefix('"') {
        return rest.split('"').next().unwrap_or("").to_string();
    }
    let lower = c.to_ascii_lowercase();
    for ext in [".exe", ".bat", ".cmd", ".lnk", ".com"] {
        if let Some(i) = lower.find(ext) {
            return c[..i + ext.len()].to_string();
        }
    }
    c.split_whitespace().next().unwrap_or("").to_string()
}

pub fn list_startup() -> Result<Vec<StartupItem>, String> {
    if demo() {
        let mk = |name: &str, cmd: &str, enabled: bool, source: &str| StartupItem {
            name: name.into(),
            command: cmd.into(),
            enabled,
            source: source.into(),
            exe: exe_from_command(cmd),
            run_key: None,
            file: None,
            approved_key: String::new(),
            approved_name: name.into(),
            needs_admin: source.contains("all users"),
        };
        return Ok(vec![
            mk(
                "Discord",
                r#""C:\Users\me\AppData\Local\Discord\Update.exe" --processStart Discord.exe"#,
                false,
                "Registry (current user)",
            ),
            mk(
                "Microsoft OneDrive",
                r#""C:\Program Files\Microsoft OneDrive\OneDrive.exe" /background"#,
                true,
                "Registry (current user)",
            ),
            mk(
                "Steam",
                r#""C:\Program Files (x86)\Steam\steam.exe" -silent"#,
                false,
                "Registry (current user)",
            ),
            mk(
                "SecurityHealth",
                r"%windir%\system32\SecurityHealthSystray.exe",
                true,
                "Registry (all users)",
            ),
            mk(
                "Xvfb",
                "/usr/bin/Xvfb :99",
                true,
                "Startup folder (current user)",
            ),
        ]);
    }
    if !IS_WINDOWS {
        return list_startup_xdg();
    }
    let mut items = Vec::new();
    let reg_sources = [
        ("HKCU", RUN, "Run", "Registry (current user)", false),
        ("HKLM", RUN, "Run", "Registry (all users)", true),
        ("HKLM", RUN32, "Run32", "Registry (all users, 32-bit)", true),
    ];
    for (hive, key, approved_sub, label, admin) in reg_sources {
        let run_key = format!(r"{hive}\{key}");
        let approved_key = format!(r"{hive}\{APPROVED}\{approved_sub}");
        let flags = reg_values(&approved_key);
        for v in reg_values(&run_key) {
            if v.name.is_empty() {
                continue;
            }
            let enabled = flags
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(&v.name))
                .map(|f| approved_flag(&f.data))
                .unwrap_or(true);
            items.push(StartupItem {
                exe: exe_from_command(&expand_env(&v.data)),
                name: v.name.clone(),
                command: v.data.clone(),
                enabled,
                source: label.into(),
                run_key: Some(run_key.clone()),
                file: None,
                approved_key: approved_key.clone(),
                approved_name: v.name,
                needs_admin: admin,
            });
        }
    }
    let folders = [
        (
            std::env::var("APPDATA").ok(),
            "HKCU",
            "Startup folder (current user)",
            false,
        ),
        (
            std::env::var("ProgramData").ok(),
            "HKLM",
            "Startup folder (all users)",
            true,
        ),
    ];
    for (base, hive, label, admin) in folders {
        let Some(base) = base else { continue };
        let dir = Path::new(&base).join(r"Microsoft\Windows\Start Menu\Programs\Startup");
        let approved_key = format!(r"{hive}\{APPROVED}\StartupFolder");
        let flags = reg_values(&approved_key);
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let fname = entry.file_name().to_string_lossy().into_owned();
            if fname.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            let enabled = flags
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(&fname))
                .map(|f| approved_flag(&f.data))
                .unwrap_or(true);
            let path = entry.path();
            items.push(StartupItem {
                name: path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or(fname.clone()),
                command: path.display().to_string(),
                exe: path.display().to_string(),
                enabled,
                source: label.into(),
                run_key: None,
                file: Some(path),
                approved_key: approved_key.clone(),
                approved_name: fname,
                needs_admin: admin,
            });
        }
    }
    items.sort_by_key(|i| i.name.to_lowercase());
    Ok(items)
}

fn list_startup_xdg() -> Result<Vec<StartupItem>, String> {
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let dir = Path::new(&home).join(".config/autostart");
    let mut items = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let text = std::fs::read_to_string(e.path()).unwrap_or_default();
            let get = |k: &str| {
                text.lines()
                    .find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string()))
                    .unwrap_or_default()
            };
            let cmd = get("Exec=");
            items.push(StartupItem {
                name: Some(get("Name="))
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| e.file_name().to_string_lossy().into_owned()),
                exe: exe_from_command(&cmd),
                command: cmd,
                enabled: get("Hidden=") != "true",
                source: "XDG autostart".into(),
                run_key: None,
                file: Some(e.path()),
                approved_key: String::new(),
                approved_name: String::new(),
                needs_admin: false,
            });
        }
    }
    Ok(items)
}

/// Expand %VAR% references in a string.
pub fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(a) = rest.find('%') {
        out.push_str(&rest[..a]);
        let after = &rest[a + 1..];
        match after.find('%') {
            Some(b) => {
                let var = &after[..b];
                match std::env::var(var) {
                    Ok(v) if !var.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(var);
                        out.push('%');
                    }
                }
                rest = &after[b + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn set_startup_enabled(item: &StartupItem, enabled: bool) -> Result<String, String> {
    if !IS_WINDOWS {
        return Err("Toggling startup items is only available on Windows".into());
    }
    let data = if enabled {
        "020000000000000000000000"
    } else {
        "030000000000000000000000"
    };
    run(
        "reg",
        &[
            "add",
            &item.approved_key,
            "/v",
            &item.approved_name,
            "/t",
            "REG_BINARY",
            "/d",
            data,
            "/f",
        ],
    )
    .map(|_| {
        format!(
            "{} {}",
            item.name,
            if enabled { "enabled" } else { "disabled" }
        )
    })
}

pub fn delete_startup(item: &StartupItem) -> Result<String, String> {
    if let Some(key) = &item.run_key {
        run("reg", &["delete", key, "/v", &item.approved_name, "/f"])?;
        let _ = run(
            "reg",
            &[
                "delete",
                &item.approved_key,
                "/v",
                &item.approved_name,
                "/f",
            ],
        );
    } else if let Some(f) = &item.file {
        std::fs::remove_file(f).map_err(|e| e.to_string())?;
    }
    Ok(format!("Removed {} from startup", item.name))
}

pub fn add_startup(name: &str, cmd: &str) -> Result<String, String> {
    if name.trim().is_empty() || cmd.trim().is_empty() {
        return Err("Name and command are both required".into());
    }
    if !IS_WINDOWS {
        return Err("Adding startup items is only available on Windows".into());
    }
    run(
        "reg",
        &[
            "add",
            &format!(r"HKCU\{RUN}"),
            "/v",
            name.trim(),
            "/t",
            "REG_SZ",
            "/d",
            cmd.trim(),
            "/f",
        ],
    )
    .map(|_| format!("Added {} to startup", name.trim()))
}

// ───────────────────────────── Scheduled tasks at sign-in / boot ─────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchedTask {
    pub name: String,
    pub path: String,
    /// "Ready", "Disabled", "Running", …
    pub state: String,
    /// "Logon", "Boot" or both.
    pub trigger: String,
    pub action: String,
    pub author: String,
}

impl SchedTask {
    pub fn enabled(&self) -> bool {
        !self.state.eq_ignore_ascii_case("disabled")
    }
    pub fn is_microsoft(&self) -> bool {
        self.path.to_lowercase().starts_with("\\microsoft\\")
    }
    pub fn exe(&self) -> String {
        exe_from_command(&expand_env(&self.action))
    }
}

pub fn parse_sched_csv(text: &str) -> Vec<SchedTask> {
    let rows = parse_csv(text);
    let Some((header, body)) = rows.split_first() else {
        return Vec::new();
    };
    let col = |n: &str| header.iter().position(|h| h.eq_ignore_ascii_case(n));
    let get =
        |r: &Vec<String>, i: Option<usize>| i.and_then(|i| r.get(i)).cloned().unwrap_or_default();
    let (n, p, st, tr, ac, au) = (
        col("Name"),
        col("Path"),
        col("State"),
        col("Trigger"),
        col("Action"),
        col("Author"),
    );
    body.iter()
        .map(|r| SchedTask {
            name: get(r, n),
            path: get(r, p),
            state: get(r, st),
            trigger: get(r, tr),
            action: get(r, ac),
            author: get(r, au),
        })
        .filter(|t| !t.name.is_empty())
        .collect()
}

/// Scheduled tasks that run when you sign in or when Windows boots —
/// a common place for apps to hide their auto-start.
pub fn list_scheduled_startup() -> Result<Vec<SchedTask>, String> {
    if demo() {
        return Ok(parse_sched_csv(DEMO_TASKS));
    }
    if !IS_WINDOWS {
        return Err("Scheduled tasks are only available on Windows".into());
    }
    let mut v = crate::native::list_logon_boot_tasks()?;
    v.sort_by_key(|t| (t.is_microsoft(), t.name.to_lowercase()));
    Ok(v)
}

pub fn set_task_enabled(t: &SchedTask, enable: bool) -> Result<String, String> {
    let full = format!("{}{}", t.path, t.name);
    let flag = if enable { "/enable" } else { "/disable" };
    run("schtasks", &["/change", "/tn", &full, flag])
        .map(|_| format!("{} {}", t.name, if enable { "enabled" } else { "disabled" }))
}

/// Services that start automatically and are not part of Windows itself.
pub fn third_party_autostart(services: &[Service]) -> Vec<Service> {
    services
        .iter()
        .filter(|s| s.start_mode.starts_with("Automatic"))
        .filter(|s| {
            let p = s.path.to_lowercase();
            !p.is_empty()
                && !p.contains("\\windows\\")
                && !p.contains("\\windows defender")
                && !p.contains("\\microsoft\\")
        })
        .cloned()
        .collect()
}

// ───────────────────────────── Services ─────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct Service {
    pub name: String,
    pub display: String,
    pub state: String,
    pub start_mode: String,
    pub pid: u32,
    pub description: String,
    pub path: String,
}

/// Minimal RFC-4180 CSV parser (quoted fields, doubled quotes, embedded newlines).
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_q = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_q {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => in_q = false,
                _ => field.push(c),
            }
        } else {
            match c {
                '"' => in_q = true,
                ',' => row.push(std::mem::take(&mut field)),
                '\r' => {}
                '\n' => {
                    row.push(std::mem::take(&mut field));
                    if !(row.len() == 1 && row[0].is_empty()) {
                        rows.push(std::mem::take(&mut row));
                    } else {
                        row.clear();
                    }
                }
                _ => field.push(c),
            }
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

pub fn parse_services_csv(text: &str) -> Vec<Service> {
    let rows = parse_csv(text);
    let Some((header, body)) = rows.split_first() else {
        return Vec::new();
    };
    let col = |n: &str| header.iter().position(|h| h.eq_ignore_ascii_case(n));
    let (ci_name, ci_disp, ci_state, ci_mode, ci_pid, ci_desc, ci_path, ci_delay) = (
        col("Name"),
        col("DisplayName"),
        col("State"),
        col("StartMode"),
        col("ProcessId"),
        col("Description"),
        col("PathName"),
        col("DelayedAutoStart"),
    );
    let get =
        |r: &Vec<String>, i: Option<usize>| i.and_then(|i| r.get(i)).cloned().unwrap_or_default();
    body.iter()
        .map(|r| {
            let mut mode = get(r, ci_mode);
            if mode == "Auto" {
                mode = if get(r, ci_delay).eq_ignore_ascii_case("true") {
                    "Automatic (delayed)".into()
                } else {
                    "Automatic".into()
                };
            }
            Service {
                name: get(r, ci_name),
                display: get(r, ci_disp),
                state: get(r, ci_state),
                start_mode: mode,
                pid: get(r, ci_pid).parse().unwrap_or(0),
                description: get(r, ci_desc),
                path: get(r, ci_path),
            }
        })
        .collect()
}

pub fn list_services() -> Result<Vec<Service>, String> {
    if demo() {
        return Ok(parse_services_csv(DEMO_SERVICES));
    }
    if IS_WINDOWS {
        let mut v = crate::native::list_services()?;
        v.sort_by_key(|s| s.display.to_lowercase());
        Ok(v)
    } else {
        let out = run(
            "systemctl",
            &[
                "list-units",
                "--type=service",
                "--all",
                "--no-legend",
                "--plain",
            ],
        )?;
        Ok(out
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                let name = it.next()?.to_string();
                let _load = it.next()?;
                let _active = it.next()?;
                let sub = it.next()?.to_string();
                let desc = it.collect::<Vec<_>>().join(" ");
                Some(Service {
                    display: desc.clone(),
                    name,
                    state: sub,
                    description: desc,
                    ..Default::default()
                })
            })
            .collect())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
}

pub fn service_action(name: &str, action: ServiceAction) -> Result<String, String> {
    if IS_WINDOWS {
        // `net stop /y` also stops services that depend on this one.
        if matches!(action, ServiceAction::Stop | ServiceAction::Restart) {
            run("net", &["stop", name, "/y"])?;
        }
        if matches!(action, ServiceAction::Start | ServiceAction::Restart) {
            run("net", &["start", name])?;
        }
        Ok(format!("{name}: {action:?} succeeded"))
    } else {
        let verb = format!("{action:?}").to_lowercase();
        run("systemctl", &[&verb, name]).map(|_| format!("{name}: {action:?} succeeded"))
    }
}

pub const START_MODES: [(&str, &str); 4] = [
    ("Automatic", "auto"),
    ("Automatic (delayed)", "delayed-auto"),
    ("Manual", "demand"),
    ("Disabled", "disabled"),
];

pub fn set_service_start(name: &str, sc_mode: &str) -> Result<String, String> {
    if !IS_WINDOWS {
        return Err("Changing start type is only available on Windows".into());
    }
    run("sc", &["config", name, "start=", sc_mode]).map(|_| format!("{name}: start type updated"))
}

// ───────────────────────────── Network connections ─────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct Conn {
    pub proto: String,
    pub local: String,
    pub remote: String,
    pub state: String,
    pub pid: u32,
}

impl Conn {
    pub fn remote_host(&self) -> &str {
        split_host_port(&self.remote).0
    }
    pub fn is_external(&self) -> bool {
        let h = self.remote_host();
        !(h.is_empty()
            || h == "*"
            || h == "0.0.0.0"
            || h == "[::]"
            || h.starts_with("127.")
            || h == "[::1]"
            || h.starts_with("10.")
            || h.starts_with("192.168.")
            || h.starts_with("[fe80")
            || is_172_private(h))
    }
}

fn is_172_private(h: &str) -> bool {
    let mut it = h.split('.');
    it.next() == Some("172")
        && it
            .next()
            .and_then(|s| s.parse::<u8>().ok())
            .is_some_and(|n| (16..=31).contains(&n))
}

pub fn split_host_port(addr: &str) -> (&str, &str) {
    match addr.rfind(':') {
        Some(i) if !addr.ends_with(']') => (&addr[..i], &addr[i + 1..]),
        _ => (addr, ""),
    }
}

/// Parse `netstat -ano` output.
pub fn parse_netstat(text: &str) -> Vec<Conn> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            match f.as_slice() {
                [p, local, remote, state, pid] if p.eq_ignore_ascii_case("TCP") => Some(Conn {
                    proto: "TCP".into(),
                    local: local.to_string(),
                    remote: remote.to_string(),
                    state: state.to_string(),
                    pid: pid.parse().ok()?,
                }),
                [p, local, remote, pid] if p.eq_ignore_ascii_case("UDP") => Some(Conn {
                    proto: "UDP".into(),
                    local: local.to_string(),
                    remote: remote.to_string(),
                    state: String::new(),
                    pid: pid.parse().ok()?,
                }),
                _ => None,
            }
        })
        .collect()
}

/// Parse `ss -tunapH` output (Linux fallback).
fn parse_ss(text: &str) -> Vec<Conn> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 6 {
                return None;
            }
            let pid = l
                .find("pid=")
                .and_then(|i| l[i + 4..].split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let state = if f[1] == "UNCONN" {
                String::new()
            } else {
                f[1].to_string()
            };
            Some(Conn {
                proto: f[0].to_uppercase(),
                local: f[4].to_string(),
                remote: f[5].to_string(),
                state,
                pid,
            })
        })
        .collect()
}

pub fn list_connections() -> Result<Vec<Conn>, String> {
    if demo() {
        return Ok(parse_netstat(DEMO_NETSTAT));
    }
    if IS_WINDOWS {
        run("netstat", &["-ano"]).map(|t| parse_netstat(&t))
    } else {
        run("ss", &["-tunapH"]).map(|t| parse_ss(&t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reg_query_parsing() {
        let t = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\r\n    OneDrive    REG_SZ    \"C:\\Users\\a\\OneDrive.exe\" /background\r\n    My App Name    REG_EXPAND_SZ    %LOCALAPPDATA%\\x.exe\r\n    Empty    REG_SZ\r\n";
        let v = parse_reg_query(t);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].name, "OneDrive");
        assert_eq!(v[0].data, "\"C:\\Users\\a\\OneDrive.exe\" /background");
        assert_eq!(v[1].name, "My App Name");
        assert_eq!(v[1].kind, "REG_EXPAND_SZ");
        assert_eq!(v[2].data, "");
    }

    #[test]
    fn approved_flags() {
        assert!(approved_flag("020000000000000000000000"));
        assert!(approved_flag("060000000000000000000000"));
        assert!(!approved_flag("03000000A1B2C3D4E5F60708"));
        assert!(approved_flag(""));
    }

    #[test]
    fn exe_extraction() {
        assert_eq!(
            exe_from_command("\"C:\\Program Files\\A\\a.exe\" -x"),
            "C:\\Program Files\\A\\a.exe"
        );
        assert_eq!(
            exe_from_command("C:\\Program Files\\B\\b.exe /min"),
            "C:\\Program Files\\B\\b.exe"
        );
        assert_eq!(exe_from_command("rundll32 foo"), "rundll32");
    }

    #[test]
    fn csv_parsing() {
        let t = "\"Name\",\"DisplayName\",\"State\",\"StartMode\",\"DelayedAutoStart\",\"ProcessId\",\"Description\",\"PathName\"\r\n\"wuauserv\",\"Windows Update\",\"Running\",\"Auto\",\"True\",\"1234\",\"Says \"\"hi\"\", ok\nline2\",\"C:\\x.exe -k\"\r\n\"Spooler\",\"Print Spooler\",\"Stopped\",\"Manual\",\"False\",\"0\",\"\",\"\"\r\n";
        let s = parse_services_csv(t);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].start_mode, "Automatic (delayed)");
        assert_eq!(s[0].pid, 1234);
        assert_eq!(s[0].description, "Says \"hi\", ok\nline2");
        assert_eq!(s[1].state, "Stopped");
        assert_eq!(s[1].start_mode, "Manual");
    }

    #[test]
    fn netstat_parsing() {
        let t = "\r\nActive Connections\r\n\r\n  Proto  Local Address          Foreign Address        State           PID\r\n  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1084\r\n  TCP    192.168.1.5:52114      140.82.112.25:443      ESTABLISHED     9932\r\n  TCP    [::]:445               [::]:0                 LISTENING       4\r\n  UDP    0.0.0.0:500            *:*                                    3920\r\n";
        let c = parse_netstat(t);
        assert_eq!(c.len(), 4);
        assert_eq!(c[1].remote_host(), "140.82.112.25");
        assert!(c[1].is_external());
        assert!(!c[0].is_external());
        assert_eq!(split_host_port(&c[2].local).1, "445");
        assert_eq!(c[3].proto, "UDP");
        assert_eq!(c[3].state, "");
        assert_eq!(c[3].pid, 3920);
    }

    #[test]
    fn signature_lines() {
        let m = parse_sig_lines("C:\\A\\a.exe|Valid|Microsoft Corporation\r\nC:\\b.exe|NotSigned|\nC:\\c.exe|HashMismatch|Evil Co\n");
        assert_eq!(m.len(), 3);
        assert!(m["c:\\a\\a.exe"].verified());
        assert_eq!(m["c:\\a\\a.exe"].signer, "Microsoft Corporation");
        assert_eq!(m["c:\\b.exe"].status, "Not signed");
        assert!(m["c:\\c.exe"].status.starts_with("Tampered"));
    }

    #[test]
    fn sha256_known_value() {
        let p = std::env::temp_dir().join("tf_sha_test.txt");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(p.to_str().unwrap()).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sched_tasks_and_services() {
        let csv = "\"Name\",\"Path\",\"State\",\"Trigger\",\"Action\",\"Author\"\r\n\"OneDrive Standalone Update Task\",\"\\\",\"Ready\",\"Sign-in\",\"%localappdata%\\Microsoft\\OneDrive\\OneDriveStandaloneUpdater.exe /reporting\",\"Microsoft\"\r\n\"MicrosoftEdgeUpdateTaskMachineCore\",\"\\Microsoft\\Edge\\\",\"Disabled\",\"Sign-in, Boot\",\"C:\\Program Files (x86)\\Microsoft\\EdgeUpdate\\MicrosoftEdgeUpdate.exe /c\",\"\"\r\n";
        let t = parse_sched_csv(csv);
        assert_eq!(t.len(), 2);
        assert!(t[0].enabled() && !t[0].is_microsoft());
        assert!(!t[1].enabled() && t[1].is_microsoft());
        assert_eq!(t[1].trigger, "Sign-in, Boot");
        let svc = |n: &str, mode: &str, path: &str| Service {
            name: n.into(),
            start_mode: mode.into(),
            path: path.into(),
            ..Default::default()
        };
        let list = vec![
            svc(
                "Steam Client Service",
                "Automatic",
                "\"C:\\Program Files (x86)\\Common Files\\Steam\\steamservice.exe\" /RunAsService",
            ),
            svc(
                "wuauserv",
                "Automatic",
                "C:\\WINDOWS\\system32\\svchost.exe -k netsvcs",
            ),
            svc("Spooler", "Manual", "C:\\Program Files\\x.exe"),
        ];
        let tp = third_party_autostart(&list);
        assert_eq!(tp.len(), 1);
        assert_eq!(tp[0].name, "Steam Client Service");
    }

    #[test]
    fn env_expansion() {
        std::env::set_var("TF_TEST_VAR", "X");
        assert_eq!(expand_env("a%TF_TEST_VAR%b"), "aXb");
        assert_eq!(expand_env("100% sure"), "100% sure");
        assert_eq!(expand_env("%NOPE_TF%"), "%NOPE_TF%");
    }

    #[test]
    fn private_ranges() {
        let c = |r: &str| Conn {
            proto: "TCP".into(),
            local: "x:1".into(),
            remote: r.into(),
            state: String::new(),
            pid: 1,
        };
        assert!(!c("172.20.1.1:80").is_external());
        assert!(c("172.50.1.1:80").is_external());
        assert!(!c("[::1]:80").is_external());
    }
}
