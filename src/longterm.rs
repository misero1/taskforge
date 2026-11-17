//! Long-term history: one point every few seconds for the last 24 hours,
//! saved to disk so it survives restarts. Each point remembers which
//! process was the heaviest at that moment, so spikes can be explained later.

use std::collections::VecDeque;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const KEEP_SECS: i64 = 24 * 3600;
pub const STEP_SECS: i64 = 5;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Point {
    /// Unix seconds.
    pub t: i64,
    pub cpu: f32,
    pub mem: f32,
    pub gpu: f32,
    /// Bytes per second.
    pub net: f32,
    pub disk: f32,
    pub top_cpu: std::sync::Arc<str>,
    pub top_mem: std::sync::Arc<str>,
    pub top_gpu: std::sync::Arc<str>,
}

/// Approximate number of points in 24 hours.
const MAX_POINTS: usize = (KEEP_SECS / STEP_SECS) as usize;

#[derive(Default)]
pub struct Store {
    pub points: VecDeque<Point>,
    path: Option<PathBuf>,
    /// Points recorded since the last write.
    unsaved: Vec<Point>,
    /// Lines currently in the file (used to decide when to compact it).
    lines_in_file: usize,
}

/// What the sampler should write to disk (done outside its lock).
pub struct PendingWrite {
    pub path: PathBuf,
    pub points: Vec<Point>,
    /// Rewrite the whole file (compaction) instead of appending.
    pub rewrite: bool,
}

impl PendingWrite {
    pub fn write(&self) {
        use std::io::Write;
        if self.points.is_empty() && !self.rewrite {
            return;
        }
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut buf = String::with_capacity(self.points.len() * 120);
        for p in &self.points {
            if let Ok(line) = serde_json::to_string(p) {
                buf.push_str(&line);
                buf.push('\n');
            }
        }
        let res = if self.rewrite {
            let tmp = self.path.with_extension("jsonl.tmp");
            std::fs::write(&tmp, buf).and_then(|_| std::fs::rename(&tmp, &self.path))
        } else {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .and_then(|mut f| f.write_all(buf.as_bytes()))
        };
        if let Err(e) = res {
            crate::diag::warn(&format!("Could not save history: {e}"));
        }
    }
}

impl Store {
    /// Load from the app's data folder (`%APPDATA%\TaskForge\data` on Windows).
    /// History is stored as JSON Lines so saving only appends new points.
    pub fn load() -> Store {
        let dir = eframe::storage_dir("TaskForge");
        let path = dir.as_ref().map(|d| d.join("history.jsonl"));
        let cutoff = now() - KEEP_SECS;
        let mut points: VecDeque<Point> = VecDeque::new();
        // Older versions wrote one big JSON array.
        if let Some(legacy) = dir.as_ref().map(|d| d.join("history.json")) {
            if let Some(v) = std::fs::read_to_string(&legacy)
                .ok()
                .and_then(|s| serde_json::from_str::<Vec<Point>>(&s).ok())
            {
                points.extend(v);
            }
            let _ = std::fs::remove_file(legacy);
        }
        if let Some(text) = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
            points.extend(
                text.lines()
                    .filter_map(|l| serde_json::from_str::<Point>(l).ok()),
            );
        }
        points.retain(|p| p.t >= cutoff);
        let mut store = Store {
            points,
            path,
            unsaved: Vec::new(),
            lines_in_file: 0,
        };
        // Start each session with a compact file.
        if let Some(path) = store.path.clone() {
            PendingWrite {
                path,
                points: store.points.iter().cloned().collect(),
                rewrite: true,
            }
            .write();
            store.lines_in_file = store.points.len();
        }
        store
    }

    pub fn push(&mut self, p: Point) {
        let cutoff = p.t - KEEP_SECS;
        self.unsaved.push(p.clone());
        self.points.push_back(p);
        while self.points.front().is_some_and(|f| f.t < cutoff) {
            self.points.pop_front();
        }
    }

    /// Hand over new points for writing. Every ~2 days of appends the file is
    /// rewritten so it never grows past twice the 24-hour window.
    pub fn take_unsaved(&mut self) -> Option<PendingWrite> {
        let path = self.path.clone()?;
        if self.unsaved.is_empty() {
            return None;
        }
        self.lines_in_file += self.unsaved.len();
        if self.lines_in_file > MAX_POINTS * 2 {
            self.unsaved.clear();
            self.lines_in_file = self.points.len();
            return Some(PendingWrite {
                path,
                points: self.points.iter().cloned().collect(),
                rewrite: true,
            });
        }
        Some(PendingWrite {
            path,
            points: std::mem::take(&mut self.unsaved),
            rewrite: false,
        })
    }

    /// Points within the last `secs` seconds.
    pub fn window(&self, secs: i64) -> impl Iterator<Item = &Point> {
        let cutoff = now() - secs;
        self.points.iter().filter(move |p| p.t >= cutoff)
    }
}

pub fn now() -> i64 {
    chrono::Local::now().timestamp()
}

/// The `n` largest peaks of a metric, at least `gap` seconds apart.
pub fn peaks<'a>(
    points: &[&'a Point],
    metric: impl Fn(&Point) -> f32,
    n: usize,
    gap: i64,
) -> Vec<&'a Point> {
    let mut sorted: Vec<&Point> = points.to_vec();
    sorted.sort_by(|a, b| metric(b).total_cmp(&metric(a)));
    let mut out: Vec<&Point> = Vec::new();
    for p in sorted {
        if out.len() >= n {
            break;
        }
        if out.iter().all(|q| (q.t - p.t).abs() >= gap) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_are_spaced() {
        let pts: Vec<Point> = (0..100)
            .map(|i| Point {
                t: i * 5,
                cpu: if i == 10 || i == 11 || i == 50 {
                    90.0
                } else {
                    5.0
                },
                ..Default::default()
            })
            .collect();
        let refs: Vec<&Point> = pts.iter().collect();
        let p = peaks(&refs, |p| p.cpu, 3, 60);
        assert_eq!(p.len(), 3);
        assert!(p[0].cpu == 90.0 && p[1].cpu == 90.0);
        assert!((p[0].t - p[1].t).abs() >= 60);
    }

    #[test]
    fn push_trims_old() {
        let mut s = Store::default();
        s.push(Point {
            t: 0,
            ..Default::default()
        });
        s.push(Point {
            t: KEEP_SECS + 10,
            ..Default::default()
        });
        assert_eq!(s.points.len(), 1);
    }
}
