//! Error log and diagnostics.
//!
//! Writes a small rotating log to `%APPDATA%\TaskForge\data\taskforge.log`
//! (1 MB, previous file kept as `.old`), records crashes via a panic hook,
//! and builds a "diagnostics" report the user can copy and send.

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_BYTES: u64 = 1024 * 1024;
const RECENT: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Info,
    Warn,
    Error,
}

struct Logger {
    path: Option<PathBuf>,
    recent: VecDeque<String>,
}

static LOG: Mutex<Logger> = Mutex::new(Logger {
    path: None,
    recent: VecDeque::new(),
});

pub fn log_dir() -> Option<PathBuf> {
    eframe::storage_dir("TaskForge")
}

pub fn log_path() -> Option<PathBuf> {
    log_dir().map(|d| d.join("taskforge.log"))
}

/// Open the log file and install the crash handler. Call once at startup.
pub fn init() {
    if let Some(dir) = log_dir() {
        let _ = std::fs::create_dir_all(&dir);
    }
    if let Ok(mut l) = LOG.lock() {
        l.path = log_path();
    }
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_string();
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        let loc = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        write(
            Level::Error,
            &format!("CRASH in thread '{thread}' at {loc}: {msg}"),
        );
        default(info);
    }));
    info(&format!(
        "TaskForge {} starting on {} ({}), admin={}",
        env!("CARGO_PKG_VERSION"),
        sysinfo::System::long_os_version().unwrap_or_default(),
        std::env::consts::ARCH,
        crate::platform::is_admin()
    ));
}

pub fn info(msg: &str) {
    write(Level::Info, msg);
}
pub fn warn(msg: &str) {
    write(Level::Warn, msg);
}
pub fn write(level: Level, msg: &str) {
    let line = format!(
        "{} [{level:?}] {msg}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f")
    );
    let Ok(mut l) = LOG.lock() else { return };
    l.recent.push_back(line.clone());
    while l.recent.len() > RECENT {
        l.recent.pop_front();
    }
    let Some(path) = l.path.clone() else { return };
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}

/// Last log lines written this session (newest last).
pub fn recent(n: usize) -> Vec<String> {
    LOG.lock()
        .map(|l| l.recent.iter().rev().take(n).rev().cloned().collect())
        .unwrap_or_default()
}

/// Number of warnings / errors recorded this session.
pub fn problem_count() -> usize {
    LOG.lock()
        .map(|l| {
            l.recent
                .iter()
                .filter(|s| s.contains("[Warn]") || s.contains("[Error]"))
                .count()
        })
        .unwrap_or(0)
}

/// A plain-text report to paste into a bug report.
pub fn report(s: &crate::monitor::Snapshot) -> String {
    let mut out = String::new();
    let mut line = |k: &str, v: String| out.push_str(&format!("{k}: {v}\n"));
    line("TaskForge", env!("CARGO_PKG_VERSION").into());
    line("OS", s.os_name.clone());
    line("Kernel", s.kernel.clone());
    line(
        "CPU",
        format!("{} ({} logical)", s.cpu_brand, s.logical_cores),
    );
    line("Memory", format!("{} MB", s.mem_total / 1024 / 1024));
    line("Administrator", crate::platform::is_admin().to_string());
    line(
        "Installed",
        crate::platform::running_installed().to_string(),
    );
    line("Processes", s.procs.len().to_string());
    line("Per-app network", format!("{:?}", s.net_status));
    if s.gpus.is_empty() {
        line("GPUs", "none detected".into());
    }
    for g in &s.gpus {
        line(
            &format!("GPU {}", g.index),
            format!(
                "{} | driver {} | engines {} | dedicated {} MB | temp {:?} | fan {:?} | power {:?} | clock {:?}",
                g.name,
                g.driver,
                g.engines.iter().map(|e| e.name.as_str()).collect::<Vec<_>>().join("/"),
                g.ded_total / 1024 / 1024,
                g.temp_c,
                g.fan_rpm,
                g.power_pct,
                g.core_mhz
            ),
        );
    }
    out.push_str("\nRecent log:\n");
    for l in recent(60) {
        out.push_str(&l);
        out.push('\n');
    }
    out
}
