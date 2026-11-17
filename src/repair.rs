//! "Fix Windows" catalogue: built-in Windows diagnostic and repair commands,
//! each with a plain-English description and the symptoms it helps with.
//!
//! Command-line fixes are written to a small `.cmd` script and opened in a
//! visible console window (elevated when needed), so the user can watch the
//! real progress and output; nothing runs hidden. GUI fixes open the matching
//! Windows tool or Settings page.

use crate::platform;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cat {
    Safety,
    SystemFiles,
    Disk,
    Network,
    Security,
    Update,
    Desktop,
    Devices,
    Power,
    Crashes,
}

impl Cat {
    pub const ALL: [Cat; 10] = [
        Cat::Safety,
        Cat::SystemFiles,
        Cat::Disk,
        Cat::Network,
        Cat::Security,
        Cat::Update,
        Cat::Desktop,
        Cat::Devices,
        Cat::Power,
        Cat::Crashes,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Cat::Safety => "Safety net",
            Cat::SystemFiles => "System files & counters",
            Cat::Disk => "Drives & storage",
            Cat::Network => "Network & internet",
            Cat::Security => "Security & malware",
            Cat::Update => "Windows Update & Store",
            Cat::Desktop => "Desktop, Start & apps",
            Cat::Devices => "Display, sound & devices",
            Cat::Power => "Power, sleep & battery",
            Cat::Crashes => "Crashes, reports & recovery",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Cat::Safety => "🛡",
            Cat::SystemFiles => "📄",
            Cat::Disk => "💾",
            Cat::Network => "🌐",
            Cat::Security => "🔒",
            Cat::Update => "⟳",
            Cat::Desktop => "🖥",
            Cat::Devices => "🔊",
            Cat::Power => "🔋",
            Cat::Crashes => "⚠",
        }
    }
}

/// How a fix is carried out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// Batch lines run in a visible console window.
    Script(&'static [&'static str]),
    /// Open a Windows tool or Settings page.
    Open(&'static str),
    /// Restart explorer.exe (built in, no window).
    RestartExplorer,
    /// Press Win+Ctrl+Shift+B, which restarts the graphics driver.
    ResetGraphics,
}

#[derive(Clone, Copy, Debug)]
pub struct Fix {
    pub id: &'static str,
    pub cat: Cat,
    pub title: &'static str,
    /// What it does, in plain English.
    pub what: &'static str,
    /// Symptoms it can fix.
    pub symptoms: &'static [&'static str],
    pub action: Action,
    pub admin: bool,
    pub time: &'static str,
    pub restart: bool,
    /// Shown in a confirmation step before running.
    pub caution: Option<&'static str>,
    /// Shown at the end of the console window.
    pub after: &'static str,
    /// Good first thing to try (highlighted).
    pub recommended: bool,
}

impl Fix {
    /// The commands as the user would type them (for "Copy command").
    pub fn command_text(&self) -> String {
        match self.action {
            Action::Script(lines) => lines
                .iter()
                .filter(|l| !is_note(l))
                .copied()
                .collect::<Vec<_>>()
                .join("\r\n"),
            Action::Open(t) => format!("start {t}"),
            Action::RestartExplorer => "taskkill /f /im explorer.exe & start explorer.exe".into(),
            Action::ResetGraphics => "Win + Ctrl + Shift + B".into(),
        }
    }

    /// Does every search term appear somewhere in this fix?
    pub fn matches(&self, terms: &[String]) -> bool {
        terms.iter().all(|t| {
            let hit = |s: &str| crate::ui::processes::contains_ci(s, t);
            hit(self.title)
                || hit(self.what)
                || hit(self.cat.label())
                || self.symptoms.iter().any(|s| hit(s))
                || matches!(self.action, Action::Script(l) if l.iter().any(|x| hit(x)))
                || matches!(self.action, Action::Open(x) if hit(x))
        })
    }
}

/// Catalogue lines starting with `# ` are explanations printed for the user,
/// not commands.
fn is_note(line: &str) -> bool {
    line.starts_with("# ")
}

/// Escape text so `echo` prints it literally. cmd treats everything inside
/// double quotes literally (a caret there would be printed), so only text
/// outside quotes is escaped.
fn echo_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut quoted = false;
    for c in s.chars() {
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && matches!(c, '^' | '&' | '|' | '<' | '>' | '(' | ')') {
            out.push('^');
        }
        out.push(c);
    }
    out
}

/// Build the `.cmd` script for a fix.
pub fn script(fix: &Fix) -> Option<String> {
    let Action::Script(lines) = fix.action else {
        return None;
    };
    let bar = "echo ==================================================================";
    let mut s = String::new();
    let mut add = |l: &str| {
        s.push_str(l);
        s.push_str("\r\n");
    };
    add("@echo off");
    add(&format!("title TaskForge - {}", echo_escape(fix.title)));
    add(bar);
    add(&format!("echo   TaskForge: {}", echo_escape(fix.title)));
    add(bar);
    add("echo.");
    for l in lines {
        if let Some(note) = l.strip_prefix("# ") {
            add(&format!("echo {}", echo_escape(note)));
        } else {
            add(&format!("echo ^> {}", echo_escape(l)));
            add(l);
            add("echo.");
        }
    }
    add("echo ------------------------------------------------------------------");
    add("echo   Finished.");
    if !fix.after.is_empty() {
        add(&format!("echo   {}", echo_escape(fix.after)));
    }
    if fix.restart {
        add("echo   Restart your PC to complete this fix.");
    }
    add("echo.");
    add("echo Press any key to close this window.");
    add("pause >nul");
    Some(s)
}

/// Run a fix. Returns a message for a toast.
pub fn run(fix: &Fix) -> Result<String, String> {
    if !platform::IS_WINDOWS {
        return Err("Windows fixes only run on Windows".into());
    }
    match fix.action {
        Action::Open(target) => {
            platform::launch_tool(target)?;
            Ok(format!("Opened: {}", fix.title))
        }
        Action::RestartExplorer => platform::restart_explorer(),
        Action::ResetGraphics => {
            crate::native::reset_graphics_driver()?;
            Ok("Graphics driver restarted (the screen may have flashed)".into())
        }
        Action::Script(_) => {
            let body = script(fix).ok_or("No script")?;
            let dir = std::env::var_os("LOCALAPPDATA")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join("TaskForge")
                .join("fixes");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join(format!("{}.cmd", fix.id));
            std::fs::write(&path, body).map_err(|e| e.to_string())?;
            let admin = fix.admin && !platform::is_admin();
            crate::native::shell_execute(&path.display().to_string(), "", admin, true)?;
            Ok(format!("{}: running in a new window", fix.title))
        }
    }
}

pub fn by_id(id: &str) -> Option<&'static Fix> {
    FIXES.iter().find(|f| f.id == id)
}

const NONE: Option<&str> = None;

pub static FIXES: &[Fix] = &[
    // ───────────── Safety net ─────────────
    Fix {
        id: "restore-point",
        cat: Cat::Safety,
        title: "Create a restore point",
        what: "Saves a snapshot of Windows' system settings, drivers and installed programs so you can roll back if a fix or update makes things worse. Your personal files are not affected.",
        symptoms: &["Before trying bigger fixes", "Before installing drivers or system tweaks"],
        action: Action::Script(&[
            "powershell -NoProfile -Command \"Checkpoint-Computer -Description 'TaskForge restore point' -RestorePointType MODIFY_SETTINGS\"",
            "# If you saw an error: System Protection may be turned off (turn it on in System Properties > System Protection),",
            "# or Windows already made a restore point in the last 24 hours.",
        ]),
        admin: true,
        time: "Under a minute",
        restart: false,
        caution: NONE,
        after: "",
        recommended: true,
    },
    Fix {
        id: "system-restore",
        cat: Cat::Safety,
        title: "Roll back with System Restore",
        what: "Opens System Restore so you can return Windows to a restore point from before the problem started. Programs and drivers installed after that point are removed; your documents and photos stay.",
        symptoms: &["Problems began right after installing an app, driver or update", "PC became unstable recently and nothing else helps"],
        action: Action::Open("rstrui.exe"),
        admin: true,
        time: "15-30 min",
        restart: true,
        caution: NONE,
        after: "",
        recommended: false,
    },
    // ───────────── System files ─────────────
    Fix {
        id: "full-repair",
        cat: Cat::SystemFiles,
        title: "Full Windows repair (DISM + SFC)",
        what: "Runs the two built-in repair tools in the order Microsoft recommends: DISM first repairs Windows' own backup copy of system files (downloading clean copies from Windows Update), then SFC uses it to replace any damaged or missing system files.",
        symptoms: &[
            "Crashes or blue screens with no clear cause",
            "Built-in apps or Settings won't open",
            "Missing DLL or 'file is corrupt' errors",
            "Strange glitches after a failed update or power cut",
        ],
        action: Action::Script(&["DISM /Online /Cleanup-Image /RestoreHealth", "sfc /scannow"]),
        admin: true,
        time: "20-60 min",
        restart: true,
        caution: NONE,
        after: "Look for 'found corrupt files and successfully repaired them' or 'did not find any integrity violations'.",
        recommended: true,
    },
    Fix {
        id: "sfc",
        cat: Cat::SystemFiles,
        title: "Check system files (SFC)",
        what: "System File Checker scans every protected Windows file and replaces any that are damaged or missing with a good copy.",
        symptoms: &["Windows features not working properly", "Random app or Explorer crashes", "Missing DLL errors"],
        action: Action::Script(&["sfc /scannow"]),
        admin: true,
        time: "10-30 min",
        restart: true,
        caution: NONE,
        after: "If it says some files could not be fixed, run 'Full Windows repair (DISM + SFC)'.",
        recommended: false,
    },
    Fix {
        id: "dism-check",
        cat: Cat::SystemFiles,
        title: "Quick check of the Windows image",
        what: "A fast DISM check that reports whether Windows' component store has been flagged as damaged. Changes nothing.",
        symptoms: &["Want to know if Windows is damaged before a long repair", "Updates or features fail to install"],
        action: Action::Script(&["DISM /Online /Cleanup-Image /CheckHealth"]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "'No component store corruption detected' means the image is fine.",
        recommended: false,
    },
    Fix {
        id: "component-cleanup",
        cat: Cat::SystemFiles,
        title: "Clean up old update files",
        what: "Removes old, replaced versions of Windows components left behind by updates (the WinSxS folder). Often frees several gigabytes.",
        symptoms: &["Low space on the C: drive", "WinSxS folder is very large"],
        action: Action::Script(&["DISM /Online /Cleanup-Image /StartComponentCleanup"]),
        admin: true,
        time: "5-30 min",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    // ───────────── Drives & storage ─────────────
    Fix {
        id: "chkdsk-scan",
        cat: Cat::Disk,
        title: "Scan the system drive for errors",
        what: "Runs CHKDSK in online mode: checks the file system on your Windows drive for errors while you keep working. Reports problems without taking the drive offline.",
        symptoms: &[
            "Files or folders that won't open or vanish",
            "'The file or directory is corrupted and unreadable'",
            "Explorer freezes when opening a certain folder",
            "After a power cut or forced shutdown",
        ],
        action: Action::Script(&["chkdsk %SystemDrive% /scan"]),
        admin: true,
        time: "5-20 min",
        restart: false,
        caution: NONE,
        after: "If errors were found, run 'Fix drive errors at next restart'.",
        recommended: true,
    },
    Fix {
        id: "chkdsk-fix",
        cat: Cat::Disk,
        title: "Fix drive errors at next restart",
        what: "Schedules CHKDSK /F to repair file-system errors on the Windows drive. It runs before Windows starts, because the drive can't be repaired while it's in use.",
        symptoms: &["The scan above found errors", "Corrupted files keep appearing", "Windows says a drive needs repair"],
        action: Action::Script(&["echo Y| chkdsk %SystemDrive% /f"]),
        admin: true,
        time: "Runs at restart, 10-60 min",
        restart: true,
        caution: Some("Your next restart will take longer while the drive is checked. Don't turn the PC off during it."),
        after: "",
        recommended: false,
    },
    Fix {
        id: "drive-health",
        cat: Cat::Disk,
        title: "Check drive health",
        what: "Shows the health status Windows reads from each physical drive (SSD/HDD), plus temperature, wear and read/write error counts where the drive reports them.",
        symptoms: &["PC is very slow to open files or boot", "Clicking or grinding noises", "Frequent freezes for a few seconds", "Worried a drive is failing"],
        action: Action::Script(&[
            "powershell -NoProfile -Command \"Get-PhysicalDisk | Sort-Object DeviceId | Format-Table DeviceId,FriendlyName,MediaType,HealthStatus,OperationalStatus,@{n='Size GB';e={[math]::Round($_.Size/1GB)}} -AutoSize\"",
            "powershell -NoProfile -Command \"Get-PhysicalDisk | Sort-Object DeviceId | Get-StorageReliabilityCounter | Format-Table DeviceId,Temperature,Wear,ReadErrorsTotal,WriteErrorsTotal,PowerOnHours -AutoSize\"",
            "# Healthy means the drive reports no problems. Warning or Unhealthy: back up your files now.",
            "# Wear is the percentage of an SSD's rated life used up. Blank values mean the drive doesn't report them.",
        ]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    Fix {
        id: "temp-clean",
        cat: Cat::Disk,
        title: "Delete temporary files",
        what: "Empties your personal Temp folder, where apps and installers leave leftover files. Files still in use are skipped automatically.",
        symptoms: &["Low disk space", "Installers fail or behave oddly", "Temp folder has grown large"],
        action: Action::Script(&[
            "if not defined TEMP exit /b 1",
            "cd /d \"%TEMP%\" || exit /b 1",
            "del /f /s /q \"%TEMP%\\*\" >nul 2>&1",
            "for /d %%i in (\"%TEMP%\\*\") do rd /s /q \"%%i\" 2>nul",
            "# Temporary files removed. Files that were in use were skipped.",
        ]),
        admin: false,
        time: "Under a minute",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    Fix {
        id: "disk-cleanup",
        cat: Cat::Disk,
        title: "Disk Cleanup",
        what: "Opens Windows' Disk Cleanup to remove temporary files, the recycle bin, old update files and other things you don't need. Click 'Clean up system files' for the most space.",
        symptoms: &["Low disk space", "'You're running out of space' warnings"],
        action: Action::Open("cleanmgr.exe"),
        admin: false,
        time: "A few minutes",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    Fix {
        id: "optimize-drives",
        cat: Cat::Disk,
        title: "Optimize all drives",
        what: "Runs the same optimisation Windows schedules weekly: sends TRIM to SSDs so they stay fast, and defragments hard drives.",
        symptoms: &["Hard drive has become slow over time", "Optimisation hasn't run in a long time"],
        action: Action::Script(&["defrag /C /O /U"]),
        admin: true,
        time: "1-30 min",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    // ───────────── Network ─────────────
    Fix {
        id: "net-test",
        cat: Cat::Network,
        title: "Internet connection test",
        what: "Checks step by step whether your PC has a network address, can reach the internet, and can look up website names (DNS), then tells you which part is failing.",
        symptoms: &["No internet", "Some websites won't load", "Not sure if it's the PC, the router or the provider"],
        action: Action::Script(&[
            "# Step 1: this PC's network address and router (gateway):",
            "ipconfig | findstr /i \"IPv4 Gateway\"",
            "# Step 2: can this PC reach the internet?",
            "ping -n 4 1.1.1.1",
            "# Step 3: can it look up website names (DNS)?",
            "nslookup www.microsoft.com",
            "# ------------------------------------------------------------------",
            "#  - Address starts with 169.254 or is missing: the PC isn't getting a connection. Try 'Renew IP address'.",
            "#  - Step 2 fails: the problem is the Wi-Fi/cable, router or internet provider. Restart the router.",
            "#  - Step 2 works but step 3 fails: a DNS problem. Try 'Flush DNS cache' or 'Reset network settings'.",
        ]),
        admin: false,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "",
        recommended: true,
    },
    Fix {
        id: "flush-dns",
        cat: Cat::Network,
        title: "Flush DNS cache",
        what: "Clears the list of website addresses Windows has remembered, so it looks them up fresh.",
        symptoms: &["One website won't load but others do", "'DNS_PROBE_FINISHED' or 'server not found' errors", "A site moved and you still get the old one"],
        action: Action::Script(&["ipconfig /flushdns"]),
        admin: false,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    Fix {
        id: "renew-ip",
        cat: Cat::Network,
        title: "Renew IP address",
        what: "Releases your network address and asks the router for a new one. Your connection drops for a few seconds.",
        symptoms: &["Connected to Wi-Fi but 'No internet'", "'Limited connectivity'", "IP address starts with 169.254", "Internet stopped after the router restarted"],
        action: Action::Script(&["ipconfig /release", "ipconfig /renew", "ipconfig /flushdns"]),
        admin: true,
        time: "Under a minute",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    Fix {
        id: "net-reset",
        cat: Cat::Network,
        title: "Reset network settings",
        what: "Resets Winsock and TCP/IP to their defaults and clears the DNS cache. Undoes damage left by uninstalled VPNs, antivirus or 'internet booster' software.",
        symptoms: &["No internet in any app though Wi-Fi is connected", "Internet works in some apps but not others", "Problems after removing a VPN or antivirus", "'Windows Sockets registry entries are missing'"],
        action: Action::Script(&["netsh winsock reset", "netsh int ip reset", "ipconfig /flushdns"]),
        admin: true,
        time: "Under a minute",
        restart: true,
        caution: Some("Custom network settings such as a static IP address or proxy may need re-entering, and some VPN or security apps may need reinstalling."),
        after: "",
        recommended: false,
    },
    Fix {
        id: "wifi-report",
        cat: Cat::Network,
        title: "Wi-Fi report",
        what: "Creates Windows' Wi-Fi report: every connection, disconnection and error from the last 3 days, with the reason Windows recorded. Opens in your browser.",
        symptoms: &["Wi-Fi keeps dropping out", "Disconnects after sleep", "Slow or unreliable wireless"],
        action: Action::Script(&[
            "netsh wlan show wlanreport",
            "start \"\" \"%ProgramData%\\Microsoft\\Windows\\WlanReport\\wlan-report-latest.html\"",
        ]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "The report opened in your browser.",
        recommended: false,
    },
    Fix {
        id: "network-reset-full",
        cat: Cat::Network,
        title: "Full network reset (Settings)",
        what: "Opens Network & internet settings. Choose 'Network reset' (on Windows 11 it's under Advanced network settings) to remove and reinstall every network adapter. Use it when the other network fixes didn't help.",
        symptoms: &["Nothing else fixed the internet", "Network adapter missing or broken"],
        action: Action::Open("ms-settings:network-status"),
        admin: true,
        time: "5 min + restart",
        restart: true,
        caution: Some("Network reset forgets saved Wi-Fi passwords and removes VPN adapters."),
        after: "",
        recommended: false,
    },
    // ───────────── Windows Update & Store ─────────────
    Fix {
        id: "wu-reset",
        cat: Cat::Update,
        title: "Reset Windows Update",
        what: "Stops the update services, sets aside Windows Update's download cache and signature database (SoftwareDistribution and catroot2) so they're rebuilt fresh, then starts the services again.",
        symptoms: &["Updates stuck downloading or at 0%", "The same update fails again and again", "Errors like 0x80070002, 0x8024xxxx or 0x800f0922", "'Something went wrong' on the Update page"],
        action: Action::Script(&[
            "net stop wuauserv /y",
            "net stop bits /y",
            "net stop cryptsvc /y",
            "if exist \"%windir%\\SoftwareDistribution.old\" rd /s /q \"%windir%\\SoftwareDistribution.old\"",
            "ren \"%windir%\\SoftwareDistribution\" SoftwareDistribution.old",
            "if exist \"%windir%\\System32\\catroot2.old\" rd /s /q \"%windir%\\System32\\catroot2.old\"",
            "ren \"%windir%\\System32\\catroot2\" catroot2.old",
            "net start cryptsvc",
            "net start bits",
            "net start wuauserv",
        ]),
        admin: true,
        time: "1-2 min",
        restart: true,
        caution: Some("The list on the Update history page is cleared (installed updates stay installed). Pending downloads start again from scratch."),
        after: "After restarting, open Windows Update and check for updates again.",
        recommended: true,
    },
    Fix {
        id: "store-reset",
        cat: Cat::Update,
        title: "Reset Microsoft Store cache",
        what: "Runs WSReset, which clears the Microsoft Store's cache without changing your apps or account. The Store opens when it's done.",
        symptoms: &["Microsoft Store won't open or closes straight away", "Store downloads stuck at 'Pending'", "Apps won't update from the Store"],
        action: Action::Open("wsreset.exe"),
        admin: false,
        time: "Under a minute",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    Fix {
        id: "troubleshooters",
        cat: Cat::Update,
        title: "Windows troubleshooters",
        what: "Opens Windows' built-in troubleshooters (Windows Update, audio, network, printer, Bluetooth and more), which find and fix common problems automatically.",
        symptoms: &["Not sure what's wrong", "Want Windows to diagnose a specific feature"],
        action: Action::Open("ms-settings:troubleshoot"),
        admin: false,
        time: "A few minutes",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    // ───────────── Desktop, Start & search ─────────────
    Fix {
        id: "restart-explorer",
        cat: Cat::Desktop,
        title: "Restart Windows Explorer",
        what: "Restarts the process that draws the taskbar, Start menu, desktop and File Explorer windows. Open apps are not affected.",
        symptoms: &["Taskbar frozen or missing", "Start menu won't open", "Desktop icons not responding", "File Explorer stuck"],
        action: Action::RestartExplorer,
        admin: false,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "",
        recommended: true,
    },
    Fix {
        id: "icon-cache",
        cat: Cat::Desktop,
        title: "Rebuild icon and thumbnail cache",
        what: "Deletes Windows' saved icon and thumbnail images and restarts Explorer, so they're rebuilt correctly.",
        symptoms: &["Blank, white or wrong icons", "Icons show the wrong program", "Picture thumbnails missing or showing the wrong image"],
        action: Action::Script(&[
            "taskkill /f /im explorer.exe",
            "del /a /f /q \"%LocalAppData%\\IconCache.db\" 2>nul",
            "del /a /f /q \"%LocalAppData%\\Microsoft\\Windows\\Explorer\\iconcache_*.db\" 2>nul",
            "del /a /f /q \"%LocalAppData%\\Microsoft\\Windows\\Explorer\\thumbcache_*.db\" 2>nul",
            "start explorer.exe",
        ]),
        admin: false,
        time: "Seconds",
        restart: false,
        caution: Some("The taskbar and desktop disappear for a few seconds while Explorer restarts."),
        after: "",
        recommended: false,
    },
    Fix {
        id: "fix-search",
        cat: Cat::Desktop,
        title: "Fix Start menu search",
        what: "Restarts the Windows Search service and the search box process.",
        symptoms: &["Typing in Start or the taskbar search does nothing", "Search shows a blank panel", "Search results never appear"],
        action: Action::Script(&[
            "net stop wsearch /y",
            "net start wsearch",
            "taskkill /f /im SearchHost.exe 2>nul",
            "taskkill /f /im SearchApp.exe 2>nul",
            "taskkill /f /im SearchUI.exe 2>nul",
        ]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "Open Start and try searching again.",
        recommended: false,
    },
    Fix {
        id: "rebuild-index",
        cat: Cat::Desktop,
        title: "Rebuild the search index",
        what: "Opens Indexing Options. Click Advanced, then Rebuild, to re-create the index of your files from scratch. It rebuilds in the background over a few hours.",
        symptoms: &["Search can't find files you know exist", "New files never show up in search", "SearchIndexer constantly using CPU or disk"],
        action: Action::Open("control.exe srchadmin.dll"),
        admin: true,
        time: "Hours (in the background)",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    // ───────────── Display, sound & devices ─────────────
    Fix {
        id: "reset-gpu",
        cat: Cat::Devices,
        title: "Restart graphics driver",
        what: "Presses Win+Ctrl+Shift+B for you, which makes Windows restart the graphics driver. The screen goes black for a second and you hear a beep. Open apps stay open.",
        symptoms: &["Screen frozen, black or flickering", "Display broken after waking from sleep", "Graphics glitches after a game"],
        action: Action::ResetGraphics,
        admin: false,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    Fix {
        id: "audio-restart",
        cat: Cat::Devices,
        title: "Restart sound services",
        what: "Restarts Windows Audio and the Audio Endpoint Builder, the two services every app's sound goes through.",
        symptoms: &["No sound from any app", "'No audio output device is installed'", "Red X on the speaker icon", "Crackling or stuttering sound after sleep"],
        action: Action::Script(&[
            "net stop audiosrv /y",
            "net stop AudioEndpointBuilder /y",
            "net start AudioEndpointBuilder",
            "net start audiosrv",
        ]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "Sound may take a few seconds to come back.",
        recommended: true,
    },
    Fix {
        id: "print-spooler",
        cat: Cat::Devices,
        title: "Clear stuck print jobs",
        what: "Stops the Print Spooler, deletes everything waiting in the print queue, and starts it again.",
        symptoms: &["A print job is stuck and won't delete", "Nothing prints but the printer is on", "Printer shows 'Error' or 'Offline' after a failed job"],
        action: Action::Script(&[
            "net stop spooler /y",
            "del /f /q \"%SystemRoot%\\System32\\spool\\PRINTERS\\*.*\"",
            "net start spooler",
        ]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: Some("Every waiting print job is cancelled."),
        after: "",
        recommended: false,
    },
    Fix {
        id: "scan-devices",
        cat: Cat::Devices,
        title: "Re-detect hardware",
        what: "Asks Windows to scan for hardware changes, the same as 'Scan for hardware changes' in Device Manager.",
        symptoms: &["A plugged-in device isn't detected", "Device disappeared after a driver update", "USB device shows nothing"],
        action: Action::Script(&["pnputil /scan-devices"]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    Fix {
        id: "device-problems",
        cat: Cat::Devices,
        title: "Find devices with driver problems",
        what: "Lists every connected device that Windows says isn't working properly (the ones with a yellow warning in Device Manager).",
        symptoms: &["Something isn't working and you're not sure why", "Yellow warning icons in Device Manager", "Missing drivers after reinstalling Windows"],
        action: Action::Script(&[
            "powershell -NoProfile -Command \"$d = Get-PnpDevice -PresentOnly | Where-Object Status -ne 'OK'; if ($d) { $d | Format-Table Status,Class,FriendlyName -AutoSize } else { 'All devices report OK.' }\"",
            "# To fix one: open Device Manager, right-click the device, and choose Update driver or Uninstall device, then restart.",
        ]),
        admin: false,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    // ───────────── Power, sleep & battery ─────────────
    Fix {
        id: "sleep-blockers",
        cat: Cat::Power,
        title: "What's stopping sleep?",
        what: "Shows which apps or drivers are keeping the PC awake right now, what woke it last time, and which devices are allowed to wake it.",
        symptoms: &["PC won't go to sleep", "PC wakes up by itself at night", "Screen won't turn off"],
        action: Action::Script(&[
            "# What is keeping the PC awake right now:",
            "powercfg /requests",
            "# What woke the PC last time:",
            "powercfg /lastwake",
            "# Devices allowed to wake the PC:",
            "powercfg /devicequery wake_armed",
        ]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "To stop a device waking the PC: Device Manager, device Properties, Power Management tab.",
        recommended: false,
    },
    Fix {
        id: "battery-report",
        cat: Cat::Power,
        title: "Battery health report",
        what: "Creates Windows' battery report: the battery's original capacity versus what it holds now (wear), plus recent usage and estimated life. Opens in your browser.",
        symptoms: &["Battery runs out much faster than it used to", "Laptop dies at 20-30%", "Want to know if the battery needs replacing"],
        action: Action::Script(&[
            "powercfg /batteryreport /output \"%TEMP%\\battery-report.html\"",
            "start \"\" \"%TEMP%\\battery-report.html\"",
        ]),
        admin: false,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "Compare DESIGN CAPACITY with FULL CHARGE CAPACITY to see how worn the battery is.",
        recommended: false,
    },
    Fix {
        id: "energy-report",
        cat: Cat::Power,
        title: "Power efficiency report",
        what: "Watches the PC for 60 seconds and reports power problems: apps that stop the CPU idling, USB devices that won't suspend, bad power settings. Opens in your browser.",
        symptoms: &["Battery drains quickly even when idle", "Fans always running", "Laptop runs hot doing nothing"],
        action: Action::Script(&[
            "# Measuring for 60 seconds. Leave the PC alone until it finishes...",
            "powercfg /energy /duration 60 /output \"%TEMP%\\energy-report.html\"",
            "start \"\" \"%TEMP%\\energy-report.html\"",
        ]),
        admin: true,
        time: "About 1 min",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    Fix {
        id: "fast-startup-off",
        cat: Cat::Power,
        title: "Turn off Fast Startup",
        what: "Fast Startup makes 'Shut down' a kind of hibernation, so drivers and errors can carry over between sessions. Turning it off makes every shutdown a clean one. Hibernate still works.",
        symptoms: &["Problems that 'Restart' fixes but 'Shut down' doesn't", "Uptime keeps growing even though you shut down", "Drive locked or unreadable when dual-booting", "Devices not working after power-on"],
        action: Action::Script(&[
            "reg add \"HKLM\\SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Power\" /v HiberbootEnabled /t REG_DWORD /d 0 /f",
        ]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: Some("Starting from shut down will be a few seconds slower. To turn it back on: Control Panel > Power Options > Choose what the power buttons do."),
        after: "Fast Startup is now off.",
        recommended: false,
    },
    // ───────────── Crashes & hardware ─────────────
    Fix {
        id: "recent-crashes",
        cat: Cat::Crashes,
        title: "Show recent crashes and blue screens",
        what: "Lists the latest unexpected shutdowns, blue screens and power losses from the Windows event log, with dates, so you can see how often it happens and when it started.",
        symptoms: &["PC restarts by itself", "Blue screen of death (BSOD)", "PC turns off suddenly"],
        action: Action::Script(&[
            "powershell -NoProfile -Command \"$e = Get-WinEvent -FilterHashtable @{LogName='System'; Id=41,1001,6008} -MaxEvents 20 -ErrorAction SilentlyContinue; if ($e) { $e | Format-Table TimeCreated,Id,@{n='What happened';e={($_.Message -split [Environment]::NewLine)[0]}} -AutoSize -Wrap } else { 'No crashes or unexpected shutdowns found.' }\"",
            "# 41 / 6008 = shut down without warning (crash, freeze, power loss or held power button).",
            "# 1001 = blue screen; the 'bugcheck' code in the text helps look up the cause.",
            "# Frequent 41s with no blue screen often point to power supply, overheating or a driver.",
        ]),
        admin: false,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "",
        recommended: true,
    },
    Fix {
        id: "memory-test",
        cat: Cat::Crashes,
        title: "Test memory (RAM)",
        what: "Opens Windows Memory Diagnostic, which restarts the PC and tests your RAM for faults before Windows loads. Results appear after you sign in again.",
        symptoms: &["Random blue screens", "Crashes in lots of different apps", "Files getting corrupted", "Freezes that don't follow any pattern"],
        action: Action::Open("mdsched.exe"),
        admin: true,
        time: "15-30 min + restart",
        restart: true,
        caution: NONE,
        after: "",
        recommended: false,
    },
    Fix {
        id: "time-sync",
        cat: Cat::Crashes,
        title: "Sync the clock",
        what: "Starts the Windows Time service and forces it to correct the clock against the internet time server right now.",
        symptoms: &["Clock is wrong", "Browser says 'Your clock is ahead/behind'", "Certificate or security errors on every website", "Apps can't sign in"],
        action: Action::Script(&["net start w32time", "w32tm /resync /force"]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "If the clock is still wrong after a restart, the motherboard (CMOS) battery may be flat.",
        recommended: false,
    },
    // ═════════════ Added in 1.4 ═════════════
    // ───────────── Desktop, Start & apps ─────────────
    Fix {
        id: "reset-start",
        cat: Cat::Desktop,
        title: "Reset the Start menu",
        what: "Closes the Start menu process and re-registers the Start menu app, so Windows rebuilds it from a clean copy. Your pinned apps normally stay.",
        symptoms: &["Start menu won't open when you click it", "Start opens blank or freezes", "Pinned apps or the 'All apps' list disappeared"],
        action: Action::Script(&[
            "taskkill /f /im StartMenuExperienceHost.exe 2>nul",
            "powershell -NoProfile -Command \"Get-AppxPackage -AllUsers Microsoft.Windows.StartMenuExperienceHost | ForEach-Object { Add-AppxPackage -DisableDevelopmentMode -Register (Join-Path $_.InstallLocation 'AppXManifest.xml') }\"",
            "taskkill /f /im StartMenuExperienceHost.exe 2>nul",
        ]),
        admin: true,
        time: "Under a minute",
        restart: false,
        caution: NONE,
        after: "Click Start to try it. If it's still broken, try 'Re-register built-in apps'.",
        recommended: true,
    },
    Fix {
        id: "reregister-apps",
        cat: Cat::Desktop,
        title: "Re-register built-in apps",
        what: "Re-registers every installed Windows app (Settings, Photos, Calculator, Notepad, Store apps...) from the files already on your PC, repairing broken app registrations. Nothing is downloaded and no app data is deleted.",
        symptoms: &["Settings, Photos, Calculator or Notepad won't open", "Built-in apps open and close straight away", "App icons do nothing when clicked"],
        action: Action::Script(&[
            "# Re-registering apps. This can take several minutes; apps that are open are skipped.",
            "powershell -NoProfile -Command \"$ok = 0; $skip = 0; Get-AppxPackage -AllUsers | Where-Object { $_.InstallLocation -and (Test-Path (Join-Path $_.InstallLocation 'AppXManifest.xml')) } | ForEach-Object { try { Add-AppxPackage -DisableDevelopmentMode -Register (Join-Path $_.InstallLocation 'AppXManifest.xml') -ErrorAction Stop; $ok++ } catch { $skip++ } }; 'Re-registered ' + $ok + ' apps (' + $skip + ' skipped because they were in use or are system-protected).'\"",
        ]),
        admin: true,
        time: "5-15 min",
        restart: true,
        caution: Some("Close your open apps first. Some apps can't be re-registered while they're in use and are skipped; that's normal."),
        after: "",
        recommended: true,
    },
    Fix {
        id: "fix-clipboard",
        cat: Cat::Desktop,
        title: "Fix copy & paste",
        what: "Restarts the clipboard services (including the one remote desktop uses) and clears unpinned clipboard history (Win+V). Pinned clipboard items are kept.",
        symptoms: &["Copy and paste stopped working", "Ctrl+C / Ctrl+V do nothing", "Win+V clipboard history is empty or stuck", "Pasting gives old content"],
        action: Action::Script(&[
            "taskkill /f /im rdpclip.exe 2>nul",
            "powershell -NoProfile -Command \"Get-Service cbdhsvc_* -ErrorAction SilentlyContinue | Restart-Service -Force -ErrorAction SilentlyContinue; try { $null = [Windows.ApplicationModel.DataTransfer.Clipboard, Windows.ApplicationModel.DataTransfer, ContentType = WindowsRuntime]; [Windows.ApplicationModel.DataTransfer.Clipboard]::ClearHistory() | Out-Null; 'Clipboard history cleared.' } catch { 'Clipboard history could not be cleared (it may be turned off).' }\"",
        ]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "Try copying and pasting again.",
        recommended: true,
    },
    Fix {
        id: "explorer-history",
        cat: Cat::Desktop,
        title: "Clear File Explorer history & Quick Access",
        what: "Deletes File Explorer's recent files, Quick Access and jump-list data, plus the address-bar, search and Run box histories, then restarts Explorer.",
        symptoms: &["File Explorer is slow to open", "Quick Access / Home is stuck loading", "Quick Access shows files you deleted", "Recent files list is wrong or cluttered"],
        action: Action::Script(&[
            "taskkill /f /im explorer.exe",
            "del /f /q \"%AppData%\\Microsoft\\Windows\\Recent\\AutomaticDestinations\\*\" 2>nul",
            "del /f /q \"%AppData%\\Microsoft\\Windows\\Recent\\CustomDestinations\\*\" 2>nul",
            "del /f /q \"%AppData%\\Microsoft\\Windows\\Recent\\*.lnk\" 2>nul",
            "reg delete \"HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\TypedPaths\" /f 2>nul",
            "reg delete \"HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\WordWheelQuery\" /f 2>nul",
            "reg delete \"HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\RunMRU\" /f 2>nul",
            "start explorer.exe",
        ]),
        admin: false,
        time: "Seconds",
        restart: false,
        caution: Some("Folders you pinned to Quick Access and the recent items in taskbar jump lists are cleared. Your actual files are not touched."),
        after: "",
        recommended: false,
    },
    Fix {
        id: "font-cache",
        cat: Cat::Desktop,
        title: "Rebuild the font cache",
        what: "Stops the Windows Font Cache service, deletes its saved font data, and starts it again so fonts are re-read from the Fonts folder.",
        symptoms: &["Text shows as boxes or strange symbols", "Fonts look garbled or wrong in apps", "Newly installed fonts don't appear", "Apps are slow to show text after startup"],
        action: Action::Script(&[
            "net stop FontCache /y",
            "net stop FontCache3.0.0.0 /y 2>nul",
            "del /a /f /q \"%WinDir%\\ServiceProfiles\\LocalService\\AppData\\Local\\FontCache\\*FontCache*\" 2>nul",
            "del /a /f /q \"%WinDir%\\System32\\FNTCACHE.DAT\" 2>nul",
            "net start FontCache",
        ]),
        admin: true,
        time: "Seconds",
        restart: true,
        caution: NONE,
        after: "",
        recommended: false,
    },
    // ───────────── Network ─────────────
    Fix {
        id: "wifi-passwords",
        cat: Cat::Network,
        title: "Find a saved Wi-Fi password",
        what: "Opens Windows' Wi-Fi settings, where Windows 11 can show the password of any network you've saved: choose 'Manage known networks', pick the network, then 'View' next to 'View Wi-Fi security key'. On Windows 10, open the network you're connected to under Network and Sharing Center, then Wireless Properties > Security > Show characters.",
        symptoms: &["Forgot the Wi-Fi password", "Need the password for a phone, TV or guest", "Want to check which networks are saved"],
        action: Action::Open("ms-settings:network-wifisettings"),
        admin: false,
        time: "A minute",
        restart: false,
        caution: NONE,
        after: "",
        recommended: true,
    },
    Fix {
        id: "reset-proxy",
        cat: Cat::Network,
        title: "Reset proxy settings",
        what: "Turns off any manual proxy and automatic proxy script for your account, and resets the system-wide (WinHTTP) proxy that Windows services use.",
        symptoms: &["'Can't connect to the proxy server'", "ERR_PROXY_CONNECTION_FAILED in the browser", "Internet works in some apps but not the browser", "Problems after malware or removing a VPN"],
        action: Action::Script(&[
            "reg add \"HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\" /v ProxyEnable /t REG_DWORD /d 0 /f",
            "reg delete \"HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\" /v ProxyServer /f 2>nul",
            "reg delete \"HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\" /v AutoConfigURL /f 2>nul",
            "netsh winhttp reset proxy",
        ]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: Some("If your work or school network needs a proxy, you'll have to set it up again."),
        after: "Close and reopen your browser.",
        recommended: true,
    },
    Fix {
        id: "reset-hosts",
        cat: Cat::Network,
        title: "Reset the hosts file",
        what: "Replaces the hosts file (a list that can force website names to go to specific addresses) with Windows' empty default. Your old file is kept as a backup next to it.",
        symptoms: &["Websites redirect to the wrong place", "Certain sites or updates are blocked", "Problems after adware or a 'cracked' program", "Antivirus can't update"],
        action: Action::Script(&[
            "attrib -r -s -h \"%WinDir%\\System32\\drivers\\etc\\hosts\" 2>nul",
            "copy /y \"%WinDir%\\System32\\drivers\\etc\\hosts\" \"%WinDir%\\System32\\drivers\\etc\\hosts.taskforge-backup\" >nul",
            "echo # Default hosts file restored by TaskForge. The old file is hosts.taskforge-backup>\"%WinDir%\\System32\\drivers\\etc\\hosts\"",
            "ipconfig /flushdns",
        ]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: Some("Any custom entries (for ad-blocking or development) are removed. A backup is saved as hosts.taskforge-backup in the same folder."),
        after: "",
        recommended: false,
    },
    Fix {
        id: "reset-firewall",
        cat: Cat::Network,
        title: "Reset Windows Firewall",
        what: "Puts Windows Defender Firewall back to its default settings and removes every rule added by apps or by hand.",
        symptoms: &["An app can't reach the internet even though others can", "Games or apps blocked after changing firewall rules", "Network sharing broken after removing security software"],
        action: Action::Script(&["netsh advfirewall reset"]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: Some("All custom firewall rules are deleted. Apps that need the network will ask for permission again the next time they run."),
        after: "",
        recommended: false,
    },
    Fix {
        id: "dns-cloudflare",
        cat: Cat::Network,
        title: "Use Cloudflare DNS (1.1.1.1)",
        what: "Sets your network adapters to look up website names using Cloudflare, usually the fastest public DNS, with a strong privacy policy. Undo any time with 'Use automatic DNS'.",
        symptoms: &["Websites slow to start loading", "DNS errors or 'server not found'", "Your provider's DNS is unreliable"],
        action: Action::Script(&[
            "powershell -NoProfile -Command \"Get-NetAdapter -Physical | Where-Object Status -eq 'Up' | Set-DnsClientServerAddress -ServerAddresses ('1.1.1.1','1.0.0.1','2606:4700:4700::1111','2606:4700:4700::1001')\"",
            "ipconfig /flushdns",
            "powershell -NoProfile -Command \"Get-NetAdapter -Physical | Where-Object Status -eq 'Up' | Get-DnsClientServerAddress | Format-Table InterfaceAlias,AddressFamily,ServerAddresses -AutoSize\"",
        ]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "If a hotel or work Wi-Fi sign-in page stops appearing, use 'Use automatic DNS'.",
        recommended: false,
    },
    Fix {
        id: "dns-google",
        cat: Cat::Network,
        title: "Use Google DNS (8.8.8.8)",
        what: "Sets your network adapters to look up website names using Google Public DNS, which is fast and very reliable. Undo any time with 'Use automatic DNS'.",
        symptoms: &["Websites slow to start loading", "DNS errors or 'server not found'", "Your provider's DNS is unreliable"],
        action: Action::Script(&[
            "powershell -NoProfile -Command \"Get-NetAdapter -Physical | Where-Object Status -eq 'Up' | Set-DnsClientServerAddress -ServerAddresses ('8.8.8.8','8.8.4.4','2001:4860:4860::8888','2001:4860:4860::8844')\"",
            "ipconfig /flushdns",
            "powershell -NoProfile -Command \"Get-NetAdapter -Physical | Where-Object Status -eq 'Up' | Get-DnsClientServerAddress | Format-Table InterfaceAlias,AddressFamily,ServerAddresses -AutoSize\"",
        ]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "If a hotel or work Wi-Fi sign-in page stops appearing, use 'Use automatic DNS'.",
        recommended: false,
    },
    Fix {
        id: "dns-quad9",
        cat: Cat::Network,
        title: "Use Quad9 DNS (9.9.9.9)",
        what: "Sets your network adapters to look up website names using Quad9, a non-profit DNS that also blocks known malware and phishing sites. Undo any time with 'Use automatic DNS'.",
        symptoms: &["Want extra protection from malicious websites", "DNS errors or 'server not found'", "Your provider's DNS is unreliable"],
        action: Action::Script(&[
            "powershell -NoProfile -Command \"Get-NetAdapter -Physical | Where-Object Status -eq 'Up' | Set-DnsClientServerAddress -ServerAddresses ('9.9.9.9','149.112.112.112','2620:fe::fe','2620:fe::9')\"",
            "ipconfig /flushdns",
            "powershell -NoProfile -Command \"Get-NetAdapter -Physical | Where-Object Status -eq 'Up' | Get-DnsClientServerAddress | Format-Table InterfaceAlias,AddressFamily,ServerAddresses -AutoSize\"",
        ]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "If a hotel or work Wi-Fi sign-in page stops appearing, use 'Use automatic DNS'.",
        recommended: false,
    },
    Fix {
        id: "dns-auto",
        cat: Cat::Network,
        title: "Use automatic DNS",
        what: "Puts your network adapters back to the DNS servers your router or provider hands out (Windows' default). Undoes the Cloudflare, Google or Quad9 fixes.",
        symptoms: &["Undo a DNS change", "Hotel, airport or work Wi-Fi sign-in page won't appear", "Work or school sites only work on their own DNS"],
        action: Action::Script(&[
            "powershell -NoProfile -Command \"Get-NetAdapter -Physical | Set-DnsClientServerAddress -ResetServerAddresses\"",
            "ipconfig /flushdns",
            "powershell -NoProfile -Command \"Get-NetAdapter -Physical | Where-Object Status -eq 'Up' | Get-DnsClientServerAddress | Format-Table InterfaceAlias,AddressFamily,ServerAddresses -AutoSize\"",
        ]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    // ───────────── Security ─────────────
    Fix {
        id: "defender-quick",
        cat: Cat::Security,
        title: "Microsoft Defender quick scan",
        what: "Updates Microsoft Defender's virus definitions, then scans the places malware most often hides (running programs, startup locations, system folders).",
        symptoms: &["Strange pop-ups or ads", "Browser home page or search engine changed by itself", "Unknown programs appeared", "PC suddenly much slower"],
        action: Action::Script(&[
            "\"%ProgramFiles%\\Windows Defender\\MpCmdRun.exe\" -SignatureUpdate",
            "\"%ProgramFiles%\\Windows Defender\\MpCmdRun.exe\" -Scan -ScanType 1",
            "# If Defender reports it is turned off, another antivirus is in charge; scan with that one instead.",
        ]),
        admin: true,
        time: "2-10 min",
        restart: false,
        caution: NONE,
        after: "Threats found are quarantined automatically. See Windows Security > Protection history.",
        recommended: true,
    },
    Fix {
        id: "defender-offline",
        cat: Cat::Security,
        title: "Microsoft Defender offline scan",
        what: "Restarts the PC into a special Defender scan that runs before Windows loads, so it can remove malware that hides from or blocks normal scans.",
        symptoms: &["Malware keeps coming back after removal", "Antivirus won't run or gets switched off", "Suspected rootkit"],
        action: Action::Script(&["powershell -NoProfile -Command \"Start-MpWDOScan\""]),
        admin: true,
        time: "About 15 min",
        restart: true,
        caution: Some("Your PC restarts straight away. Save your work and close apps first."),
        after: "",
        recommended: false,
    },
    // ───────────── System & storage ─────────────
    Fix {
        id: "perf-counters",
        cat: Cat::SystemFiles,
        title: "Rebuild performance counters",
        what: "Rebuilds Windows' performance counters, the system that reports CPU, disk and GPU usage to Task Manager, Performance Monitor and TaskForge.",
        symptoms: &["Graphs blank or missing in Task Manager", "Performance Monitor shows errors", "TaskForge's GPU or disk stats show nothing", "'Unable to add these counters' errors"],
        action: Action::Script(&[
            "cd /d \"%WinDir%\\System32\"",
            "lodctr /R",
            "if exist \"%WinDir%\\SysWOW64\\lodctr.exe\" cd /d \"%WinDir%\\SysWOW64\"",
            "if exist \"%WinDir%\\SysWOW64\\lodctr.exe\" lodctr /R",
            "winmgmt /resyncperf",
        ]),
        admin: true,
        time: "Under a minute",
        restart: false,
        caution: NONE,
        after: "Restart TaskForge and Task Manager to see the counters again.",
        recommended: true,
    },
    Fix {
        id: "do-cache",
        cat: Cat::Disk,
        title: "Clear the Delivery Optimization cache",
        what: "Deletes the update and Store files Windows keeps so it can share them with other PCs. They're only a cache; Windows re-downloads anything it needs.",
        symptoms: &["Low disk space", "Windows Update or Store downloads stuck", "Delivery Optimization folder using lots of space"],
        action: Action::Script(&["powershell -NoProfile -Command \"Delete-DeliveryOptimizationCache -Force; 'Delivery Optimization cache cleared.'\""]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    // ───────────── Devices ─────────────
    Fix {
        id: "bluetooth",
        cat: Cat::Devices,
        title: "Restart Bluetooth",
        what: "Restarts the Bluetooth Support Service and your account's Bluetooth service, which handle pairing and connections.",
        symptoms: &["Bluetooth headphones, mouse or keyboard won't connect", "Paired devices show 'Not connected'", "Bluetooth toggle missing or greyed out", "Bluetooth stopped after sleep"],
        action: Action::Script(&[
            "net stop bthserv /y",
            "net start bthserv",
            "powershell -NoProfile -Command \"Get-Service BluetoothUserService_* -ErrorAction SilentlyContinue | Restart-Service -Force -ErrorAction SilentlyContinue\"",
        ]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "Turn the device off and on, then try connecting again. Still failing? Remove it in Settings and pair it again.",
        recommended: true,
    },
    // ───────────── Reports & recovery ─────────────
    Fix {
        id: "perf-report",
        cat: Cat::Crashes,
        title: "System performance report",
        what: "Runs Windows' built-in 60-second system diagnostics and opens a report with warnings, resource use, and checks on hardware, drivers and settings.",
        symptoms: &["PC feels slow and you don't know why", "Want a full health report", "Tech support asked for diagnostics"],
        action: Action::Script(&[
            "# The report window opens and collects data for 60 seconds.",
            "perfmon /report",
        ]),
        admin: true,
        time: "About 1 min",
        restart: false,
        caution: NONE,
        after: "Check the Warnings section at the top of the report first.",
        recommended: true,
    },
    Fix {
        id: "system-info-file",
        cat: Cat::Crashes,
        title: "Save system info to your desktop",
        what: "Writes a text file to your desktop with your Windows version, hardware, installed updates, every driver, drives and network adapters, then opens it. Ready to send to tech support or a help forum.",
        symptoms: &["Tech support asked for your system details", "Posting on a help forum", "Want a record of your drivers and hardware"],
        action: Action::Script(&[
            "# Collecting system information. This can take a minute...",
            "powershell -NoProfile -Command \"$f = Join-Path ([Environment]::GetFolderPath('Desktop')) 'TaskForge-system-info.txt'; & { '=== TaskForge system information ==='; Get-Date; ''; '=== System ==='; systeminfo; '=== Drives ==='; Get-PhysicalDisk | Format-Table FriendlyName,MediaType,HealthStatus,Size -AutoSize | Out-String; '=== Network adapters ==='; Get-NetAdapter | Format-Table Name,InterfaceDescription,Status,LinkSpeed -AutoSize | Out-String; '=== Drivers ==='; driverquery /v /fo list } | Out-File -Encoding utf8 $f; 'Saved to ' + $f; Invoke-Item -LiteralPath $f\"",
        ]),
        admin: false,
        time: "About 1 min",
        restart: false,
        caution: NONE,
        after: "",
        recommended: true,
    },
    Fix {
        id: "crash-dumps",
        cat: Cat::Crashes,
        title: "List blue-screen dump files",
        what: "Lists the crash dump files Windows saves after each blue screen, newest first, with dates and where they are. A dump file records which driver caused the crash.",
        symptoms: &["Blue screens and you want to know what's causing them", "Tech support asked for crash dumps", "Checking if old crashes are still happening"],
        action: Action::Script(&[
            "powershell -NoProfile -Command \"$d = Get-ChildItem (Join-Path $env:SystemRoot 'Minidump') -Filter *.dmp -ErrorAction SilentlyContinue; $m = Get-Item (Join-Path $env:SystemRoot 'MEMORY.DMP') -ErrorAction SilentlyContinue; $all = @($d) + @($m) | Where-Object { $_ }; if ($all) { $all | Sort-Object LastWriteTime -Descending | Format-Table LastWriteTime,@{n='Size MB';e={[math]::Round($_.Length/1MB,1)}},FullName -AutoSize } else { 'No crash dump files found.' }\"",
            "# To read one: open it in WinDbg (free in the Microsoft Store) and type !analyze -v, or use NirSoft BlueScreenView.",
        ]),
        admin: true,
        time: "Seconds",
        restart: false,
        caution: NONE,
        after: "",
        recommended: false,
    },
    Fix {
        id: "recovery-menu",
        cat: Cat::Crashes,
        title: "Restart into the recovery menu",
        what: "Restarts the PC into Windows' blue recovery menu, where you can start in Safe Mode, run Startup Repair, uninstall the latest update, or use System Restore.",
        symptoms: &["Need Safe Mode", "Windows won't start properly or keeps crashing on startup", "A bad update or driver broke Windows", "Need Startup Repair"],
        action: Action::Script(&["shutdown /r /o /t 0"]),
        admin: true,
        time: "Restart",
        restart: true,
        caution: Some("Your PC restarts immediately, so save your work first. In the menu, choose Troubleshoot > Advanced options, then Startup Settings (for Safe Mode), Startup Repair, or Uninstall Updates."),
        after: "",
        recommended: true,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_unique_and_text_present() {
        let mut ids: Vec<_> = FIXES.iter().map(|f| f.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), FIXES.len());
        for f in FIXES {
            assert!(!f.what.is_empty() && !f.symptoms.is_empty(), "{}", f.id);
            assert!(by_id(f.id).is_some());
        }
        for c in Cat::ALL {
            assert!(FIXES.iter().any(|f| f.cat == c), "empty category {c:?}");
        }
    }

    #[test]
    fn scripts_are_ascii_and_escaped() {
        for f in FIXES {
            if let Some(s) = script(f) {
                assert!(s.is_ascii(), "{} has non-ASCII text", f.id);
                assert!(s.starts_with("@echo off\r\n"));
                assert!(s.ends_with("pause >nul\r\n"));
                // Every echoed command has its pipes/redirects escaped.
                for l in s.lines().filter(|l| l.starts_with("echo ^> ")) {
                    let body = &l[8..];
                    let b = body.as_bytes();
                    let mut quoted = false;
                    for (i, c) in b.iter().enumerate() {
                        if *c == b'"' {
                            quoted = !quoted;
                        } else if !quoted && matches!(c, b'|' | b'&' | b'<' | b'>') {
                            assert!(i > 0 && b[i - 1] == b'^', "unescaped in {}: {l}", f.id);
                        }
                    }
                }
            }
        }
        let f = by_id("chkdsk-fix").unwrap();
        let s = script(f).unwrap();
        assert!(s.contains(
            "echo ^> echo Y^| chkdsk %SystemDrive% /f\r\necho Y| chkdsk %SystemDrive% /f\r\n"
        ));
        assert!(s.contains("Restart your PC"));
    }

    #[test]
    fn notes_are_not_announced_or_copied() {
        let f = by_id("net-test").unwrap();
        let s = script(f).unwrap();
        assert!(!s.contains("echo ^> echo Step"));
        assert!(!f.command_text().contains("Step 1"));
        assert!(f.command_text().contains("ping -n 4 1.1.1.1"));
        assert!(s.contains("echo Step 1: this PC's network address and router ^(gateway^):"));
        // Pipes inside quotes are left alone; outside they're escaped.
        let s = script(by_id("drive-health").unwrap()).unwrap();
        assert!(s.contains("\"Get-PhysicalDisk | Sort-Object"));
        assert!(script(by_id("temp-clean").unwrap())
            .unwrap()
            .contains("echo ^> cd /d \"%TEMP%\" ^|^| exit /b 1"));
    }

    #[test]
    fn search_by_symptom() {
        let q = |s: &str| -> Vec<&str> {
            let t: Vec<String> = s.split_whitespace().map(|x| x.to_lowercase()).collect();
            FIXES
                .iter()
                .filter(|f| f.matches(&t))
                .map(|f| f.id)
                .collect()
        };
        assert!(q("no sound").contains(&"audio-restart"));
        assert!(q("blue screen").contains(&"recent-crashes"));
        assert!(q("update stuck").contains(&"wu-reset"));
        assert!(q("sfc").contains(&"sfc"));
        assert!(q("zzzz nothing").is_empty());
    }

    #[test]
    fn added_fixes_present() {
        for id in [
            "reset-start",
            "reregister-apps",
            "fix-clipboard",
            "explorer-history",
            "font-cache",
            "wifi-passwords",
            "reset-proxy",
            "reset-hosts",
            "reset-firewall",
            "dns-cloudflare",
            "dns-google",
            "dns-quad9",
            "dns-auto",
            "defender-quick",
            "defender-offline",
            "perf-counters",
            "do-cache",
            "bluetooth",
            "perf-report",
            "system-info-file",
            "crash-dumps",
            "recovery-menu",
        ] {
            assert!(by_id(id).is_some(), "{id}");
        }
        let s = script(by_id("reset-hosts").unwrap()).unwrap();
        // The backup is made before the file is overwritten.
        assert!(
            s.find("hosts.taskforge-backup\" >nul\r\n").unwrap()
                < s.find("TaskForge. The old file").unwrap()
        );
        let q: Vec<String> = vec!["quad9".into()];
        assert!(by_id("dns-quad9").unwrap().matches(&q));
    }

    #[test]
    fn temp_clean_is_guarded() {
        let f = by_id("temp-clean").unwrap();
        let Action::Script(lines) = f.action else {
            panic!()
        };
        assert!(lines[0].starts_with("if not defined TEMP exit"));
        assert!(lines[1].contains("|| exit"));
    }
}
