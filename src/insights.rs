//! Long-running insights computed on the sampler thread:
//!
//! * **Memory-leak detection**: one memory sample per process per minute (up to
//!   6 hours). A process is a suspect when its memory has grown steadily (good
//!   straight-line fit), substantially (≥ 100 MB and ≥ 25 %), and is still
//!   growing in the most recent half of the window.
//! * **Startup impact**: CPU time and disk I/O of processes launched in the
//!   first 10 minutes after boot, measured over their first 60 seconds — the
//!   same idea and thresholds Windows uses for its "Startup impact" rating.

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::monitor::ProcInfo;

const LEAK_STEP_SECS: i64 = 60;
const LEAK_KEEP: usize = 360;
const LEAK_MIN_SAMPLES: usize = 15;
const LEAK_MIN_GROWTH: u64 = 100 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct LeakSuspect {
    pub pid: u32,
    pub name: String,
    pub start_mem: u64,
    pub now_mem: u64,
    pub mb_per_hour: f32,
    pub minutes: u32,
    /// Straight-line fit quality, 0–1.
    pub fit: f32,
}

struct Series {
    name: String,
    samples: VecDeque<(i64, u64)>,
}

#[derive(Default)]
pub struct LeakTracker {
    series: HashMap<(u32, u64), Series>,
    last: i64,
}

/// Least-squares slope (units per second) and R² of (t, v) points.
pub fn linear_fit(pts: &[(i64, u64)]) -> Option<(f64, f64)> {
    let n = pts.len() as f64;
    if pts.len() < 3 {
        return None;
    }
    let t0 = pts[0].0 as f64;
    let (mut sx, mut sy, mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for &(t, v) in pts {
        let (x, y) = (t as f64 - t0, v as f64);
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
        syy += y * y;
    }
    let denom = n * sxx - sx * sx;
    if denom.abs() < f64::EPSILON {
        return None;
    }
    let slope = (n * sxy - sx * sy) / denom;
    let num = n * sxy - sx * sy;
    let den = (n * sxx - sx * sx) * (n * syy - sy * sy);
    let r2 = if den <= 0.0 { 0.0 } else { (num * num) / den };
    Some((slope, r2))
}

impl LeakTracker {
    pub fn update(&mut self, now: i64, procs: &[ProcInfo]) {
        if now - self.last < LEAK_STEP_SECS {
            return;
        }
        self.last = now;
        let alive: std::collections::HashSet<(u32, u64)> =
            procs.iter().map(|p| (p.pid, p.start_time)).collect();
        self.series.retain(|k, _| alive.contains(k));
        for p in procs {
            if p.mem == 0 {
                continue;
            }
            let s = self
                .series
                .entry((p.pid, p.start_time))
                .or_insert_with(|| Series {
                    name: p.name.to_string(),
                    samples: VecDeque::new(),
                });
            s.samples.push_back((now, p.mem));
            while s.samples.len() > LEAK_KEEP {
                s.samples.pop_front();
            }
        }
    }

    /// Memory trend in MB per hour for one process, once there is enough data.
    pub fn trend(&self, pid: u32, start_time: u64) -> Option<f32> {
        let s = self.series.get(&(pid, start_time))?;
        if s.samples.len() < 5 {
            return None;
        }
        let pts: Vec<(i64, u64)> = s.samples.iter().copied().collect();
        linear_fit(&pts).map(|(slope, _)| (slope * 3600.0 / 1024.0 / 1024.0) as f32)
    }

    pub fn suspects(&self) -> Vec<LeakSuspect> {
        let mut out: Vec<LeakSuspect> = self
            .series
            .iter()
            .filter_map(|((pid, _), s)| analyze(*pid, &s.name, &s.samples))
            .collect();
        out.sort_by(|a, b| b.mb_per_hour.total_cmp(&a.mb_per_hour));
        out
    }
}

fn analyze(pid: u32, name: &str, samples: &VecDeque<(i64, u64)>) -> Option<LeakSuspect> {
    if samples.len() < LEAK_MIN_SAMPLES {
        return None;
    }
    let pts: Vec<(i64, u64)> = samples.iter().copied().collect();
    let first = pts.first()?.1;
    let last = pts.last()?.1;
    if last < first + LEAK_MIN_GROWTH || (last as f64) < first as f64 * 1.25 {
        return None;
    }
    let (slope, r2) = linear_fit(&pts)?;
    let (recent_slope, _) = linear_fit(&pts[pts.len() / 2..])?;
    if slope <= 0.0 || recent_slope <= 0.0 || r2 < 0.85 {
        return None;
    }
    Some(LeakSuspect {
        pid,
        name: name.to_string(),
        start_mem: first,
        now_mem: last,
        mb_per_hour: (slope * 3600.0 / 1024.0 / 1024.0) as f32,
        minutes: ((pts.last()?.0 - pts[0].0) / 60) as u32,
        fit: r2 as f32,
    })
}

// ───────────────────────────── Startup impact ─────────────────────────────

const IMPACT_WINDOW_SECS: u64 = 60;
const IMPACT_BOOT_WINDOW_SECS: u64 = 600;
const IMPACT_FIRST_SEEN_MAX_AGE: u64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImpactLevel {
    Low,
    Medium,
    High,
}

impl ImpactLevel {
    pub fn label(self) -> &'static str {
        match self {
            ImpactLevel::Low => "Low",
            ImpactLevel::Medium => "Medium",
            ImpactLevel::High => "High",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Impact {
    /// CPU time used in the first 60 s, milliseconds.
    pub cpu_ms: u64,
    /// Disk bytes read + written in the first 60 s.
    pub disk_bytes: u64,
    /// Seconds after boot the app started.
    pub after_boot_s: u64,
    /// When it was measured (unix seconds).
    pub measured: i64,
    pub level: ImpactLevel,
}

/// Windows' Startup-impact thresholds (CPU time and disk I/O during startup).
pub fn rate(cpu_ms: u64, disk_bytes: u64) -> ImpactLevel {
    if cpu_ms > 1000 || disk_bytes > 3 * 1024 * 1024 {
        ImpactLevel::High
    } else if cpu_ms > 300 || disk_bytes > 300 * 1024 {
        ImpactLevel::Medium
    } else {
        ImpactLevel::Low
    }
}

struct Watch {
    name: String,
    start: u64,
    cpu_ms: f64,
    disk: f64,
}

#[derive(Default)]
pub struct ImpactTracker {
    watching: HashMap<(u32, u64), Watch>,
    seen: std::collections::HashSet<(u32, u64)>,
    /// Keyed by lower-case process name (e.g. "onedrive.exe").
    pub results: HashMap<String, Impact>,
    dirty: bool,
}

impl ImpactTracker {
    pub fn load() -> ImpactTracker {
        let results = eframe::storage_dir("TaskForge")
            .map(|d| d.join("startup_impact.json"))
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        ImpactTracker {
            results,
            ..Default::default()
        }
    }

    /// `ncpu` converts the machine-normalised CPU % back to per-core time.
    pub fn update(&mut self, procs: &[ProcInfo], ncpu: f32, dt: f64, boot_time: u64, now: u64) {
        for p in procs {
            let key = (p.pid, p.start_time);
            if self.seen.insert(key) {
                let age = now.saturating_sub(p.start_time);
                let after_boot = p.start_time.saturating_sub(boot_time);
                if boot_time > 0
                    && after_boot <= IMPACT_BOOT_WINDOW_SECS
                    && age <= IMPACT_FIRST_SEEN_MAX_AGE
                {
                    self.watching.insert(
                        key,
                        Watch {
                            name: p.name.to_string(),
                            start: p.start_time,
                            cpu_ms: 0.0,
                            disk: 0.0,
                        },
                    );
                }
            }
            if let Some(w) = self.watching.get_mut(&key) {
                w.cpu_ms += (p.cpu * ncpu / 100.0) as f64 * dt * 1000.0;
                w.disk += (p.disk_read + p.disk_write) as f64 * dt;
            }
        }
        let alive: std::collections::HashSet<(u32, u64)> =
            procs.iter().map(|p| (p.pid, p.start_time)).collect();
        self.seen.retain(|k| alive.contains(k));
        let mut done = Vec::new();
        for (k, w) in &self.watching {
            if now.saturating_sub(w.start) >= IMPACT_WINDOW_SECS || !alive.contains(k) {
                done.push(*k);
            }
        }
        for k in done {
            if let Some(w) = self.watching.remove(&k) {
                let (cpu_ms, disk) = (w.cpu_ms as u64, w.disk as u64);
                self.results.insert(
                    w.name.to_lowercase(),
                    Impact {
                        cpu_ms,
                        disk_bytes: disk,
                        after_boot_s: w.start.saturating_sub(boot_time),
                        measured: now as i64,
                        level: rate(cpu_ms, disk),
                    },
                );
                self.dirty = true;
            }
        }
    }

    /// Serialise the results if they changed since the last call.
    pub fn to_json(&mut self) -> Option<String> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        serde_json::to_string(&self.results).ok()
    }

    pub fn write(json: &str) {
        if let Some(dir) = eframe::storage_dir("TaskForge") {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("startup_impact.json"), json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(vals: &[u64]) -> VecDeque<(i64, u64)> {
        vals.iter()
            .enumerate()
            .map(|(i, v)| (i as i64 * 60, *v))
            .collect()
    }

    #[test]
    fn fit_of_straight_line() {
        let (slope, r2) = linear_fit(&[(0, 10), (10, 20), (20, 30)]).unwrap();
        assert!((slope - 1.0).abs() < 1e-9);
        assert!((r2 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn steady_growth_is_a_leak() {
        let mb = 1024 * 1024;
        let vals: Vec<u64> = (0..30).map(|i| (200 + i * 8) * mb).collect(); // +8 MB/min
        let s = analyze(1, "leaky.exe", &series(&vals)).expect("should flag");
        assert!((s.mb_per_hour - 480.0).abs() < 1.0);
        assert!(s.fit > 0.99);
    }

    #[test]
    fn flat_or_noisy_is_not_a_leak() {
        let mb = 1024 * 1024;
        let flat: Vec<u64> = (0..30).map(|_| 500 * mb).collect();
        assert!(analyze(1, "a", &series(&flat)).is_none());
        // Big jump then plateau: grew, but not still growing.
        let step: Vec<u64> = (0..30)
            .map(|i| if i < 5 { 200 * mb } else { 700 * mb })
            .collect();
        assert!(analyze(1, "b", &series(&step)).is_none());
        // Too little data.
        let short: Vec<u64> = (0..10).map(|i| (200 + i * 50) * mb).collect();
        assert!(analyze(1, "c", &series(&short)).is_none());
    }

    #[test]
    fn impact_thresholds() {
        assert_eq!(rate(1500, 0), ImpactLevel::High);
        assert_eq!(rate(0, 4 * 1024 * 1024), ImpactLevel::High);
        assert_eq!(rate(500, 0), ImpactLevel::Medium);
        assert_eq!(rate(100, 100 * 1024), ImpactLevel::Low);
    }

    #[test]
    fn impact_measures_first_minute_after_boot() {
        let mut t = ImpactTracker::default();
        let boot = 1000;
        let p = ProcInfo {
            pid: 5,
            name: "OneDrive.exe".into(),
            start_time: boot + 30,
            cpu: 25.0,
            disk_read: 100_000,
            ..Default::default()
        };
        // 4 logical CPUs: 25% of machine = 100% of one core = 1000 ms per second.
        for s in 0..60 {
            t.update(&[p.clone()], 4.0, 1.0, boot, boot + 31 + s);
        }
        let r = t.results.get("onedrive.exe").expect("measured");
        assert!(r.cpu_ms >= 55_000);
        assert_eq!(r.level, ImpactLevel::High);
        assert_eq!(r.after_boot_s, 30);
    }
}
