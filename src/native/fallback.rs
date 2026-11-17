//! Non-Windows implementations, used for development and testing.
//!
//! Process control uses signals / procfs. GPU and per-app network data are
//! Windows-only; with `TASKFORGE_DEMO=1` they are simulated so the UI can be
//! exercised anywhere.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::{
    GpuAdapter, GpuEngine, GpuProc, GpuSample, Locker, NetStatus, ThreadAction, ThreadInfo,
    WindowInfo,
};
use crate::platform::{self, Priority};

fn demo() -> bool {
    std::env::var_os("TASKFORGE_DEMO").is_some()
}

pub fn suspend(pid: u32) -> Result<(), String> {
    platform::run("kill", &["-STOP", &pid.to_string()]).map(|_| ())
}

pub fn resume(pid: u32) -> Result<(), String> {
    platform::run("kill", &["-CONT", &pid.to_string()]).map(|_| ())
}

pub fn set_priority(pid: u32, p: Priority) -> Result<(), String> {
    platform::set_priority(pid, p).map(|_| ())
}
// (platform::set_priority uses `renice` directly on non-Windows, so this does not recurse.)

pub fn get_priority(pid: u32) -> Result<Priority, String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
    let after = stat.rsplit_once(')').map(|x| x.1).unwrap_or("");
    let nice: i32 = after
        .split_whitespace()
        .nth(16)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Ok(match nice {
        n if n <= -15 => Priority::Realtime,
        n if n <= -8 => Priority::High,
        n if n < 0 => Priority::AboveNormal,
        0 => Priority::Normal,
        n if n < 15 => Priority::BelowNormal,
        _ => Priority::Idle,
    })
}

pub fn get_affinity(_pid: u32) -> Result<(u64, u64), String> {
    Err("Affinity editing is only available on Windows".into())
}

pub fn set_affinity(_pid: u32, _mask: u64) -> Result<(), String> {
    Err("Affinity editing is only available on Windows".into())
}

pub fn write_dump(_pid: u32, _path: &Path) -> Result<(), String> {
    Err("Dump files are only available on Windows".into())
}

pub fn threads(pid: u32) -> Result<Vec<ThreadInfo>, String> {
    let dir = format!("/proc/{pid}/task");
    let mut out = Vec::new();
    for e in std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let tid: u32 = e.file_name().to_string_lossy().parse().unwrap_or(0);
        let stat = std::fs::read_to_string(e.path().join("stat")).unwrap_or_default();
        let name = stat
            .split_once('(')
            .and_then(|x| x.1.rsplit_once(')'))
            .map(|x| x.0.to_string())
            .unwrap_or_default();
        let f: Vec<&str> = stat
            .rsplit_once(')')
            .map(|x| x.1)
            .unwrap_or("")
            .split_whitespace()
            .collect();
        let ticks: u64 = f.get(11).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0)
            + f.get(12).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        let prio: i32 = f.get(15).and_then(|s| s.parse().ok()).unwrap_or(0);
        out.push(ThreadInfo {
            tid,
            base_priority: prio,
            cpu_ms: ticks * 10,
            name,
        });
    }
    out.sort_by_key(|t| t.tid);
    Ok(out)
}

pub fn thread_action(_tid: u32, _a: ThreadAction) -> Result<(), String> {
    Err("Thread control is only available on Windows".into())
}

pub fn window_at_cursor() -> Option<WindowInfo> {
    None
}

pub fn top_windows() -> Vec<WindowInfo> {
    if demo() {
        vec![WindowInfo {
            pid: std::process::id(),
            title: "TaskForge".into(),
        }]
    } else {
        Vec::new()
    }
}

/// Scan /proc/*/fd for processes holding any of the files open.
pub fn file_lockers(paths: &[PathBuf]) -> Result<Vec<Locker>, String> {
    let files: Vec<PathBuf> = super::expand_paths(paths, 5000)
        .into_iter()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .collect();
    if files.is_empty() {
        return Err("Nothing to check".into());
    }
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc")
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(e.path().join("fd")) else {
            continue;
        };
        if fds
            .flatten()
            .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| files.contains(&t)))
        {
            let app = std::fs::read_to_string(e.path().join("comm"))
                .unwrap_or_default()
                .trim()
                .to_string();
            out.push(Locker {
                pid,
                app,
                service: String::new(),
            });
        }
    }
    Ok(out)
}

pub fn net_status_hint() -> &'static str {
    "Per-app network speed is only available on Windows."
}

pub fn shutdown_net() {}

// ───────────────────────────── Demo GPU / network ─────────────────────────────

pub struct GpuSampler {
    start: Instant,
}

impl Default for GpuSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl GpuSampler {
    pub fn new() -> Self {
        GpuSampler {
            start: Instant::now(),
        }
    }

    pub fn sample(&mut self, pids: &[(u32, &str)]) -> GpuSample {
        if !demo() {
            return GpuSample::default();
        }
        let t = self.start.elapsed().as_secs_f32();
        let wave = |p: f32, a: f32, b: f32| (a + b * (t / p).sin()).max(0.0);
        let pct = |p: f32, a: f32, b: f32| wave(p, a, b).min(100.0);
        let d3 = pct(7.0, 42.0, 30.0);
        let dec = pct(11.0, 18.0, 12.0);
        let gb = 1024u64 * 1024 * 1024;
        let main = GpuAdapter {
            index: 0,
            name: "NVIDIA GeForce RTX 4070 (demo)".into(),
            vendor: "NVIDIA".into(),
            driver: "32.0.15.6094".into(),
            integrated: false,
            util: d3.max(dec),
            engines: vec![
                GpuEngine {
                    name: "3D".into(),
                    util: d3,
                },
                GpuEngine {
                    name: "Copy".into(),
                    util: pct(5.0, 6.0, 5.0),
                },
                GpuEngine {
                    name: "Video Decode".into(),
                    util: dec,
                },
                GpuEngine {
                    name: "Video Encode".into(),
                    util: pct(13.0, 4.0, 4.0),
                },
                GpuEngine {
                    name: "Compute 0".into(),
                    util: pct(17.0, 10.0, 9.0),
                },
            ],
            ded_used: (gb as f32 * (5.2 + (t / 9.0).sin())) as u64,
            ded_total: 12 * gb,
            shared_used: 380 * 1024 * 1024,
            shared_total: 16 * gb,
            budget: 11 * gb,
            temp_c: Some(wave(23.0, 58.0, 9.0)),
            temp_max_c: Some(90.0),
            fan_rpm: Some(wave(19.0, 1450.0, 400.0) as u32),
            fan_max_rpm: Some(3300),
            power_pct: Some(wave(8.0, 48.0, 25.0)),
            core_mhz: Some(wave(6.0, 2310.0, 300.0)),
            core_max_mhz: Some(2610.0),
            mem_mhz: Some(10501.0),
            mem_max_mhz: Some(10501.0),
            voltage_mv: Some(wave(6.0, 950.0, 60.0) as u32),
            mem_bandwidth: Some((wave(7.0, 120.0, 80.0) as f64) * 1e9),
            pcie_bandwidth: Some((wave(5.0, 900.0, 600.0) as f64) * 1e6),
        };
        let igpu = GpuAdapter {
            index: 1,
            name: "Intel UHD Graphics 770 (demo)".into(),
            vendor: "Intel".into(),
            driver: "31.0.101.5186".into(),
            integrated: true,
            util: wave(9.0, 6.0, 5.0),
            engines: vec![
                GpuEngine {
                    name: "3D".into(),
                    util: wave(9.0, 6.0, 5.0),
                },
                GpuEngine {
                    name: "Copy".into(),
                    util: 0.5,
                },
                GpuEngine {
                    name: "Video Decode".into(),
                    util: 0.0,
                },
            ],
            ded_used: 0,
            ded_total: 128 * 1024 * 1024,
            shared_used: 900 * 1024 * 1024,
            shared_total: 16 * gb,
            budget: 0,
            temp_c: None,
            ..Default::default()
        };
        let mut procs = HashMap::new();
        for (i, (pid, name)) in pids.iter().enumerate().take(6) {
            let share = [0.62f32, 0.21, 0.09, 0.05, 0.02, 0.01][i];
            procs.insert(
                *pid,
                GpuProc {
                    util: d3 * share,
                    engine: format!("GPU 0 - {}", if i == 1 { "Video Decode" } else { "3D" }),
                    dedicated: (main.ded_used as f32 * share) as u64,
                    shared: 20 * 1024 * 1024,
                },
            );
            let _ = name;
        }
        GpuSample {
            adapters: vec![main, igpu],
            procs,
        }
    }
}

pub struct NetMonitor {
    start: Instant,
}

impl NetMonitor {
    pub fn start() -> Result<NetMonitor, NetStatus> {
        if demo() {
            Ok(NetMonitor {
                start: Instant::now(),
            })
        } else {
            Err(NetStatus::Unavailable("Windows only".into()))
        }
    }

    /// Simulated bytes per PID (demo only); real PIDs are filled in by the caller.
    pub fn take(&self) -> HashMap<u32, (u64, u64)> {
        let t = self.start.elapsed().as_secs_f32();
        let mut m = HashMap::new();
        // Keyed by rank; the monitor maps rank -> busiest real processes.
        m.insert(
            u32::MAX,
            (
                (180_000.0 * (1.2 + (t / 4.0).sin())) as u64,
                (24_000.0 * (1.1 + (t / 3.0).cos())) as u64,
            ),
        );
        m.insert(
            u32::MAX - 1,
            ((40_000.0 * (1.1 + (t / 6.0).sin())) as u64, 9_000),
        );
        m.insert(
            u32::MAX - 2,
            (3_000, (12_000.0 * (1.2 + (t / 2.0).sin())) as u64),
        );
        m
    }
}

pub fn reset_graphics_driver() -> Result<(), String> {
    Err("Only available on Windows".into())
}

// ── Shell, services and Task Scheduler (Windows-only; see winsys.rs) ──

pub fn shell_execute(
    _file: &str,
    _params: &str,
    _admin: bool,
    _visible: bool,
) -> Result<(), String> {
    Err("Only available on Windows".into())
}

pub fn message_box(title: &str, text: &str) {
    eprintln!("{title}: {text}");
}

pub fn programs_folder() -> Option<PathBuf> {
    None
}

pub fn desktop_folder() -> Option<PathBuf> {
    None
}

pub fn create_shortcut(
    _lnk: &Path,
    _target: &Path,
    _workdir: &Path,
    _description: &str,
) -> Result<(), String> {
    Err("Only available on Windows".into())
}

pub fn list_services() -> Result<Vec<crate::platform::Service>, String> {
    Err("Only available on Windows".into())
}

pub fn list_logon_boot_tasks() -> Result<Vec<crate::platform::SchedTask>, String> {
    Err("Only available on Windows".into())
}
