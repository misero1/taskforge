//! "Why is my PC slow?" — turns the live data into plain-language findings
//! with one-click fixes. Pure logic: no OS calls, fully unit-tested.

use std::collections::HashMap;

use crate::insights::{Impact, ImpactLevel, LeakSuspect};
use crate::monitor::{ProcInfo, Snapshot};
use crate::platform::StartupItem;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Good,
    Info,
    Warn,
    Bad,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fix {
    EndProcess {
        pid: u32,
        name: String,
    },
    LowerPriority {
        pid: u32,
        name: String,
    },
    Restart {
        pid: u32,
    },
    Resume {
        pid: u32,
        name: String,
    },
    ShowProcess(u32),
    OpenStartup,
    OpenPerformance,
    OpenRepair,
    /// A Windows tool / settings URI, e.g. "cleanmgr" or "ms-settings:storagesense".
    Launch(String),
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    pub fixes: Vec<(String, Fix)>,
}

/// Everything the health check looks at.
pub struct Inputs<'a> {
    pub snap: &'a Snapshot,
    /// Recent CPU % samples (newest last), ~1 per second.
    pub cpu_recent: &'a [f32],
    /// Recent total disk bytes/s samples.
    pub disk_recent: &'a [f32],
    pub leaks: &'a [LeakSuspect],
    pub startup: &'a [StartupItem],
    pub impacts: &'a HashMap<String, Impact>,
    pub suspended: &'a [u32],
    pub is_admin: bool,
}

fn avg(v: &[f32]) -> f32 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f32>() / v.len() as f32
    }
}

fn mb(b: u64) -> String {
    if b >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", b as f64 / 1024.0 / 1024.0 / 1024.0)
    } else {
        format!("{} MB", b / 1024 / 1024)
    }
}

fn top_by(procs: &[ProcInfo], n: usize, f: impl Fn(&ProcInfo) -> f32) -> Vec<&ProcInfo> {
    let mut v: Vec<&ProcInfo> = procs.iter().filter(|p| p.pid > 4 && f(p) > 0.0).collect();
    v.sort_by(|a, b| f(b).total_cmp(&f(a)));
    v.truncate(n);
    v
}

fn end_fixes(p: &ProcInfo) -> Vec<(String, Fix)> {
    vec![
        (
            format!("End {}", p.name),
            Fix::EndProcess {
                pid: p.pid,
                name: p.name.to_string(),
            },
        ),
        (
            "Lower its priority".into(),
            Fix::LowerPriority {
                pid: p.pid,
                name: p.name.to_string(),
            },
        ),
        ("Show".into(), Fix::ShowProcess(p.pid)),
    ]
}

pub fn diagnose(i: &Inputs) -> Vec<Finding> {
    let s = i.snap;
    let mut out = Vec::new();

    // ── CPU ──
    let recent = &i.cpu_recent[i.cpu_recent.len().saturating_sub(30)..];
    let cpu = avg(recent);
    if cpu >= 50.0 && recent.len() >= 5 {
        let top = top_by(&s.procs, 3, |p| p.cpu);
        let who = top
            .iter()
            .map(|p| format!("{} ({:.0}%)", p.name, p.cpu))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(Finding {
            severity: if cpu >= 80.0 {
                Severity::Bad
            } else {
                Severity::Warn
            },
            title: format!("The processor is busy ({cpu:.0}% over the last 30 s)"),
            detail: if who.is_empty() {
                "Several small tasks add up.".into()
            } else {
                format!("Mostly: {who}.")
            },
            fixes: top.first().map(|p| end_fixes(p)).unwrap_or_default(),
        });
    }

    // ── Memory ──
    let mem = s.mem_pct();
    if mem >= 80.0 {
        let top = top_by(&s.procs, 3, |p| p.mem as f32);
        let who = top
            .iter()
            .map(|p| format!("{} ({})", p.name, mb(p.mem)))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(Finding {
            severity: if mem >= 90.0 { Severity::Bad } else { Severity::Warn },
            title: format!("Memory is nearly full ({mem:.0}% used)"),
            detail: format!(
                "When memory runs out Windows moves data to disk, which makes everything feel slow. Biggest users: {who}."
            ),
            fixes: top.first().map(|p| end_fixes(p)).unwrap_or_default(),
        });
    }
    if s.swap_total > 0 && s.swap_pct() >= 60.0 && mem >= 70.0 {
        out.push(Finding {
            severity: Severity::Warn,
            title: format!("Heavy page-file use ({:.0}%)", s.swap_pct()),
            detail: "Windows is using the disk as extra memory. Closing apps you don't need, or adding RAM, helps.".into(),
            fixes: vec![("Open Performance".into(), Fix::OpenPerformance)],
        });
    }

    // ── Disks ──
    for d in &s.disks {
        if d.total < 4 * 1024 * 1024 * 1024 || d.removable {
            continue;
        }
        let free_pct = d.available as f64 / d.total as f64 * 100.0;
        let low = free_pct < 10.0 || d.available < 5 * 1024 * 1024 * 1024;
        if low || free_pct < 15.0 {
            out.push(Finding {
                severity: if low { Severity::Bad } else { Severity::Warn },
                title: format!(
                    "Drive {} is almost full ({} free, {free_pct:.0}%)",
                    d.mount,
                    mb(d.available)
                ),
                detail: "A nearly full drive slows down updates, the page file and many apps."
                    .into(),
                fixes: vec![
                    ("Disk Cleanup".into(), Fix::Launch("cleanmgr".into())),
                    (
                        "Storage settings".into(),
                        Fix::Launch("ms-settings:storagesense".into()),
                    ),
                ],
            });
        }
    }
    let disk = avg(&i.disk_recent[i.disk_recent.len().saturating_sub(30)..]);
    if disk >= 60.0 * 1024.0 * 1024.0 {
        let top = top_by(&s.procs, 1, |p| (p.disk_read + p.disk_write) as f32);
        out.push(Finding {
            severity: Severity::Warn,
            title: format!("The disk is very busy ({}/s)", mb(disk as u64)),
            detail: top
                .first()
                .map(|p| format!("{} is doing the most reading/writing.", p.name))
                .unwrap_or_default(),
            fixes: top.first().map(|p| end_fixes(p)).unwrap_or_default(),
        });
    }

    // ── GPU ──
    for g in &s.gpus {
        if let Some(t) = g.temp_c {
            let limit = g.temp_max_c.map(|m| m - 5.0).unwrap_or(87.0).min(90.0);
            if t >= limit {
                out.push(Finding {
                    severity: Severity::Bad,
                    title: format!("{} is running hot ({t:.0} °C)", g.name),
                    detail: "Hot GPUs slow themselves down. Check the fans are spinning and the vents are clean.".into(),
                    fixes: vec![("Open GPU view".into(), Fix::OpenPerformance)],
                });
            }
        }
        if g.ded_total > 0 && g.ded_used as f64 >= g.ded_total as f64 * 0.95 {
            let top = top_by(&s.procs, 1, |p| p.gpu_mem as f32);
            out.push(Finding {
                severity: Severity::Warn,
                title: format!("GPU memory is full on {}", g.name),
                detail: format!(
                    "{} of {} used{}. Games may stutter.",
                    mb(g.ded_used),
                    mb(g.ded_total),
                    top.first()
                        .map(|p| format!(", mostly by {}", p.name))
                        .unwrap_or_default()
                ),
                fixes: top.first().map(|p| end_fixes(p)).unwrap_or_default(),
            });
        }
    }

    // ── Memory leaks ──
    for l in i.leaks.iter().take(3) {
        out.push(Finding {
            severity: Severity::Warn,
            title: format!("{} may have a memory leak", l.name),
            detail: format!(
                "Its memory grew from {} to {} over {} min (about +{:.0} MB per hour) and is still climbing. Restarting it usually frees the memory.",
                mb(l.start_mem),
                mb(l.now_mem),
                l.minutes,
                l.mb_per_hour
            ),
            fixes: vec![
                (format!("Restart {}", l.name), Fix::Restart { pid: l.pid }),
                ("Show".into(), Fix::ShowProcess(l.pid)),
            ],
        });
    }

    // ── Startup ──
    let enabled: Vec<&StartupItem> = i.startup.iter().filter(|x| x.enabled).collect();
    let heavy: Vec<&str> = enabled
        .iter()
        .filter(|x| {
            let key = std::path::Path::new(&x.exe)
                .file_name()
                .map(|f| f.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            i.impacts
                .get(&key)
                .is_some_and(|m| m.level == ImpactLevel::High)
        })
        .map(|x| x.name.as_str())
        .collect();
    if enabled.len() > 12 || !heavy.is_empty() {
        let mut detail = format!("{} apps start when you sign in.", enabled.len());
        if !heavy.is_empty() {
            detail.push_str(&format!(" High impact: {}.", heavy.join(", ")));
        }
        detail.push_str(" Disabling ones you don't need makes sign-in faster.");
        out.push(Finding {
            severity: if heavy.len() >= 3 || enabled.len() > 20 {
                Severity::Warn
            } else {
                Severity::Info
            },
            title: "Many apps start with Windows".into(),
            detail,
            fixes: vec![("Review startup apps".into(), Fix::OpenStartup)],
        });
    }

    // ── Misc ──
    if s.uptime > 7 * 86400 {
        out.push(Finding {
            severity: Severity::Info,
            title: format!("Your PC hasn't restarted in {} days", s.uptime / 86400),
            detail: "A restart clears leaked memory and finishes installing updates.".into(),
            fixes: vec![],
        });
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for p in &s.procs {
        *counts.entry(p.name.to_lowercase()).or_default() += 1;
    }
    if let Some((name, n)) = counts
        .into_iter()
        .filter(|(_, n)| *n >= 40)
        .max_by_key(|(_, n)| *n)
    {
        out.push(Finding {
            severity: Severity::Info,
            title: format!("{n} copies of {name} are running"),
            detail: "Browsers and Electron apps start a process per tab or extension. Closing unused tabs helps.".into(),
            fixes: vec![],
        });
    }
    for pid in i.suspended {
        if let Some(p) = s.procs.iter().find(|p| p.pid == *pid) {
            out.push(Finding {
                severity: Severity::Info,
                title: format!("{} is suspended", p.name),
                detail: "You paused it earlier. It won't respond until resumed.".into(),
                fixes: vec![(
                    "Resume".into(),
                    Fix::Resume {
                        pid: p.pid,
                        name: p.name.to_string(),
                    },
                )],
            });
        }
    }
    if !i.is_admin {
        out.push(Finding {
            severity: Severity::Info,
            title: "Running without administrator rights".into(),
            detail: "Some system processes, per-app network speed and a few fixes need “Restart as admin”.".into(),
            fixes: vec![],
        });
    }

    if !out.iter().any(|f| f.severity >= Severity::Warn) {
        out.insert(
            0,
            Finding {
                severity: Severity::Good,
                title: "No problems found".into(),
                detail: format!(
                    "CPU {cpu:.0}%, memory {mem:.0}%, nothing leaking and no full drives. If it still feels slow, check the History page for spikes."
                ),
                fixes: vec![],
            },
        );
    }
    out.sort_by(|a, b| b.severity.cmp(&a.severity));
    out
}

/// 0–100 health score shown as a gauge.
pub fn score(findings: &[Finding]) -> u32 {
    let mut s: i32 = 100;
    for f in findings {
        s -= match f.severity {
            Severity::Bad => 25,
            Severity::Warn => 10,
            Severity::Info => 2,
            Severity::Good => 0,
        };
    }
    s.clamp(0, 100) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::DiskInfo;

    fn base() -> Snapshot {
        Snapshot {
            mem_total: 16 << 30,
            mem_used: 4 << 30,
            uptime: 3600,
            ..Default::default()
        }
    }

    fn run(s: &Snapshot, cpu: &[f32]) -> Vec<Finding> {
        let impacts = HashMap::new();
        diagnose(&Inputs {
            snap: s,
            cpu_recent: cpu,
            disk_recent: &[],
            leaks: &[],
            startup: &[],
            impacts: &impacts,
            suspended: &[],
            is_admin: true,
        })
    }

    #[test]
    fn healthy_machine() {
        let f = run(&base(), &[5.0; 30]);
        assert_eq!(f[0].severity, Severity::Good);
        assert_eq!(score(&f), 100);
    }

    #[test]
    fn busy_cpu_names_culprit() {
        let mut s = base();
        s.procs = vec![ProcInfo {
            pid: 100,
            name: "hog.exe".into(),
            cpu: 85.0,
            ..Default::default()
        }];
        let f = run(&s, &[90.0; 30]);
        assert_eq!(f[0].severity, Severity::Bad);
        assert!(f[0].detail.contains("hog.exe"));
        assert!(matches!(f[0].fixes[0].1, Fix::EndProcess { pid: 100, .. }));
    }

    #[test]
    fn full_memory_and_disk() {
        let mut s = base();
        s.mem_used = 15 << 30;
        s.disks = vec![DiskInfo {
            mount: "C:\\".into(),
            total: 500 << 30,
            available: 3 << 30,
            ..Default::default()
        }];
        let f = run(&s, &[]);
        assert!(f
            .iter()
            .any(|x| x.title.starts_with("Memory is nearly full") && x.severity == Severity::Bad));
        assert!(f
            .iter()
            .any(|x| x.title.contains("almost full") && x.severity == Severity::Bad));
        assert!(score(&f) <= 50);
    }
}
