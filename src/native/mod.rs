#![cfg_attr(not(windows), allow(dead_code))]
//! Direct operating-system access.
//!
//! On Windows this calls Win32 / DXGI / PDH / D3DKMT / ETW / Restart Manager
//! through the official `windows` crate (see `win.rs`, `gpu_win.rs`,
//! `net_win.rs`). Every `unsafe` block there has a SAFETY note and every handle
//! is closed by a `Drop` guard.
//!
//! On other platforms `fallback.rs` provides best-effort equivalents (procfs,
//! signals) so the app keeps working during development; with
//! `TASKFORGE_DEMO=1` it also produces sample GPU and network data.

use std::collections::HashMap;

#[cfg(windows)]
mod gpu_win;
#[cfg(windows)]
mod net_win;
#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use gpu_win::GpuSampler;
#[cfg(windows)]
pub use net_win::{shutdown as shutdown_net, NetMonitor};
#[cfg(windows)]
pub use win::*;
#[cfg(windows)]
mod winsys;
#[cfg(windows)]
pub use winsys::*;

#[cfg(not(windows))]
mod fallback;
#[cfg(not(windows))]
pub use fallback::*;

// ───────────────────────────── Shared types ─────────────────────────────

/// One GPU engine type (3D, Copy, Video Decode, …) on an adapter.
#[derive(Clone, Debug, Default)]
pub struct GpuEngine {
    pub name: String,
    /// 0–100 %, the busiest engine of this type.
    pub util: f32,
}

/// A graphics adapter and its live readings. Sensor values are `None`
/// when the driver doesn't report them.
#[derive(Clone, Debug, Default)]
pub struct GpuAdapter {
    #[allow(dead_code)]
    pub index: usize,
    pub name: String,
    pub vendor: String,
    pub driver: String,
    pub integrated: bool,
    /// Overall utilisation = busiest engine (Task Manager's rule).
    pub util: f32,
    pub engines: Vec<GpuEngine>,
    pub ded_used: u64,
    pub ded_total: u64,
    pub shared_used: u64,
    pub shared_total: u64,
    /// Memory the OS currently lets apps use on this adapter (DXGI budget).
    pub budget: u64,
    pub temp_c: Option<f32>,
    pub temp_max_c: Option<f32>,
    pub fan_rpm: Option<u32>,
    pub fan_max_rpm: Option<u32>,
    /// Board power as a percentage of its limit.
    pub power_pct: Option<f32>,
    pub core_mhz: Option<f32>,
    pub core_max_mhz: Option<f32>,
    pub mem_mhz: Option<f32>,
    pub mem_max_mhz: Option<f32>,
    pub voltage_mv: Option<u32>,
    /// Bytes per second.
    pub mem_bandwidth: Option<f64>,
    pub pcie_bandwidth: Option<f64>,
}

/// Per-process GPU usage.
#[derive(Clone, Debug, Default)]
pub struct GpuProc {
    /// Busiest engine across all GPUs, 0–100 %.
    pub util: f32,
    /// e.g. "GPU 0 - 3D".
    pub engine: String,
    pub dedicated: u64,
    pub shared: u64,
}

#[derive(Clone, Debug, Default)]
pub struct GpuSample {
    pub adapters: Vec<GpuAdapter>,
    pub procs: HashMap<u32, GpuProc>,
}

#[derive(Clone, Debug)]
pub struct ThreadInfo {
    pub tid: u32,
    pub base_priority: i32,
    /// Total CPU time (kernel + user) in milliseconds.
    pub cpu_ms: u64,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadAction {
    Suspend,
    Resume,
    Kill,
}

#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub pid: u32,
    pub title: String,
}

/// A process that has a file open, from the Restart Manager.
#[derive(Clone, Debug)]
pub struct Locker {
    pub pid: u32,
    pub app: String,
    pub service: String,
}

/// Status of per-app network monitoring.
#[derive(Clone, Debug, PartialEq)]
pub enum NetStatus {
    Running,
    NeedsAdmin,
    Unavailable(String),
}

/// Parsed pieces of a GPU performance-counter instance name, e.g.
/// `pid_1234_luid_0x00000000_0x0000D1A6_phys_0_eng_3_engtype_VideoDecode`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuInstance<'a> {
    pub pid: Option<u32>,
    pub luid: Option<(u32, u32)>,
    pub phys: u32,
    pub eng: Option<u32>,
    /// Borrowed from the instance name (no allocation).
    pub engtype: &'a str,
}

pub fn parse_gpu_instance(name: &str) -> GpuInstance<'_> {
    let mut out = GpuInstance::default();
    // Everything after "engtype_" is the engine type and may itself contain '_'.
    let head = match name.find("engtype_") {
        Some(i) => {
            out.engtype = &name[i + 8..];
            &name[..i]
        }
        None => name,
    };
    let hex =
        |s: &str| u32::from_str_radix(s.trim_start_matches("0x").trim_start_matches("0X"), 16).ok();
    let mut it = head.split('_');
    while let Some(tok) = it.next() {
        match tok {
            "pid" => out.pid = it.next().and_then(|s| s.parse().ok()),
            "luid" => {
                if let (Some(h), Some(l)) = (it.next().and_then(hex), it.next().and_then(hex)) {
                    out.luid = Some((h, l));
                }
            }
            "phys" => out.phys = it.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            "eng" => out.eng = it.next().and_then(|s| s.parse().ok()),
            _ => {}
        }
    }
    out
}

/// Friendly engine names ("VideoDecode" → "Video Decode").
pub fn engine_label(raw: &str) -> String {
    match raw {
        "" => "Other".into(),
        "3D" => "3D".into(),
        "Copy" => "Copy".into(),
        "VideoDecode" => "Video Decode".into(),
        "VideoEncode" => "Video Encode".into(),
        "VideoProcessing" => "Video Processing".into(),
        "LegacyOverlay" => "Legacy Overlay".into(),
        "Security" => "Security".into(),
        other => {
            // Split CamelCase and turn "Compute_0" into "Compute 0".
            let mut s = String::new();
            let mut prev_lower = false;
            for c in other.replace('_', " ").chars() {
                if c.is_uppercase() && prev_lower {
                    s.push(' ');
                }
                prev_lower = c.is_lowercase();
                s.push(c);
            }
            s
        }
    }
}

pub fn vendor_name(id: u32) -> &'static str {
    match id {
        0x10DE => "NVIDIA",
        0x1002 | 0x1022 => "AMD",
        0x8086 => "Intel",
        0x1414 => "Microsoft",
        0x5143 => "Qualcomm",
        0x13B5 => "ARM",
        _ => "Unknown",
    }
}

/// Format the UMD driver version DXGI reports as a.b.c.d.
pub fn driver_version(v: i64) -> String {
    let v = v as u64;
    format!(
        "{}.{}.{}.{}",
        v >> 48,
        (v >> 32) & 0xFFFF,
        (v >> 16) & 0xFFFF,
        v & 0xFFFF
    )
}

/// Expand folders into the files inside them (depth-first, up to `limit` files).
pub fn expand_paths(paths: &[std::path::PathBuf], limit: usize) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = paths.to_vec();
    while let Some(p) = stack.pop() {
        if out.len() >= limit {
            break;
        }
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&p) {
                stack.extend(rd.flatten().map(|e| e.path()));
            }
        } else {
            out.push(p);
        }
    }
    out
}

/// Shell-style `*` / `?` wildcard match, case-insensitive (used by rules).
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    // Fast, allocation-free path for ASCII (process names nearly always are).
    if pattern.is_ascii() && text.is_ascii() {
        return wildcard_bytes(pattern.as_bytes(), text.as_bytes(), |a, b| {
            a.eq_ignore_ascii_case(&b)
        });
    }
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    wildcard_bytes(&p, &t, |a, b| a == b)
}

/// Iterative glob matcher with single-star backtracking (linear in practice).
fn wildcard_bytes<T: Copy + PartialEq + From<u8>>(
    p: &[T],
    t: &[T],
    eq: impl Fn(T, T) -> bool,
) -> bool {
    let (star, any) = (T::from(b'*'), T::from(b'?'));
    let (mut pi, mut ti, mut star_at, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && p[pi] != star && (p[pi] == any || eq(p[pi], t[ti])) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == star {
            star_at = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star_at {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == star {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_instance_names() {
        let e = parse_gpu_instance(
            "pid_1234_luid_0x00000000_0x0000D1A6_phys_0_eng_3_engtype_VideoDecode",
        );
        assert_eq!(e.pid, Some(1234));
        assert_eq!(e.luid, Some((0, 0xD1A6)));
        assert_eq!(e.eng, Some(3));
        assert_eq!(e.engtype, "VideoDecode");
        let c =
            parse_gpu_instance("pid_99_luid_0x00000001_0x0000ABCD_phys_1_eng_9_engtype_Compute_1");
        assert_eq!(c.phys, 1);
        assert_eq!(c.engtype, "Compute_1");
        let m = parse_gpu_instance("luid_0x00000000_0x0000D1A6_phys_0");
        assert_eq!(m.pid, None);
        assert_eq!(m.luid, Some((0, 0xD1A6)));
        assert_eq!(m.eng, None);
    }

    #[test]
    fn labels() {
        assert_eq!(engine_label("VideoDecode"), "Video Decode");
        assert_eq!(engine_label("Compute_0"), "Compute 0");
        assert_eq!(engine_label("GDI Render"), "GDI Render");
    }

    #[test]
    fn driver_versions() {
        // 31.0.15.3699 packed as 16-bit fields
        let v: i64 = (31i64 << 48) | (0 << 32) | (15 << 16) | 3699;
        assert_eq!(driver_version(v), "31.0.15.3699");
    }

    #[test]
    fn wildcards() {
        assert!(wildcard_match("chrome*", "chrome.exe"));
        assert!(wildcard_match("*.EXE", "steam.exe"));
        assert!(wildcard_match("note?ad.exe", "notepad.exe"));
        assert!(!wildcard_match("chrome", "chrome.exe"));
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("*UPDATER*", "GoogleUpdater.exe"));
        assert!(wildcard_match("über*", "Überwachung.exe"));
        assert!(!wildcard_match("a*b", "a"));
    }
}
