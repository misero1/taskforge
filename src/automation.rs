//! Background automation: rules and load alerts.
//!
//! Runs on the sampler thread after every snapshot, so it keeps working while
//! the window is minimised or hidden in the tray (egui stops calling `update`
//! for hidden windows). Messages for the user are queued as `Notice`s that the
//! UI turns into toasts when it next draws.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::monitor::{push_capped, EventKind, LogEvent, Snapshot, LOG_CAP};
use crate::platform::{self, Priority};
use crate::rules::{self, Effect, Rule, RuleState};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutoConfig {
    pub rules: Vec<Rule>,
    pub alerts_enabled: bool,
    pub alert_cpu: f32,
    pub alert_mem: f32,
    pub alert_secs: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum NoticeKind {
    Info,
    Alert,
    Ok,
    Err,
}

#[derive(Clone, Debug)]
pub struct Notice {
    pub kind: NoticeKind,
    pub text: String,
}

#[derive(Default)]
pub struct Automation {
    cfg: AutoConfig,
    rule_state: RuleState,
    cpu_since: Option<Instant>,
    mem_since: Option<Instant>,
    cpu_alerted: bool,
    mem_alerted: bool,
    pub notices: Vec<Notice>,
}

impl Automation {
    /// Update the configuration; rule state resets when the rules change.
    pub fn configure(&mut self, cfg: AutoConfig) {
        if cfg.rules != self.cfg.rules {
            self.rule_state.reset();
        }
        self.cfg = cfg;
    }

    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    fn notice(&mut self, kind: NoticeKind, text: impl Into<String>) {
        self.notices.push(Notice {
            kind,
            text: text.into(),
        });
        if self.notices.len() > 20 {
            self.notices.remove(0);
        }
    }

    pub fn on_snapshot(&mut self, s: &Snapshot, log: &mut VecDeque<LogEvent>) {
        let mut ev = |kind: EventKind, pid: u32, name: &str, detail: String| {
            push_capped(
                log,
                LogEvent {
                    time: chrono::Local::now(),
                    kind,
                    pid,
                    name: name.to_string(),
                    detail,
                },
                LOG_CAP,
            );
        };

        // ── Rules ──
        if self.cfg.rules.iter().any(|r| r.enabled) {
            let fired = rules::evaluate(
                &mut self.rule_state,
                &self.cfg.rules,
                &s.procs,
                Instant::now(),
            );
            for f in fired {
                let res: Result<String, String> = match &f.effect {
                    Effect::Priority(p) => {
                        crate::native::set_priority(f.pid, *p).map(|_| format!("Rule: {}", f.rule))
                    }
                    Effect::Affinity(m) => {
                        crate::native::set_affinity(f.pid, *m).map(|_| format!("Rule: {}", f.rule))
                    }
                    Effect::Kill => {
                        platform::kill(f.pid, false).map(|_| format!("Blocked by rule: {}", f.rule))
                    }
                    Effect::Tame => crate::native::set_priority(f.pid, Priority::BelowNormal)
                        .map(|_| "Tamed: lowered priority after sustained high CPU".to_string()),
                    Effect::Notify(msg) => {
                        ev(EventKind::Alert, f.pid, &f.name, msg.clone());
                        self.notice(NoticeKind::Alert, format!("⚡ {msg}"));
                        continue;
                    }
                };
                match res {
                    Ok(msg) => {
                        if matches!(f.effect, Effect::Kill | Effect::Tame) {
                            self.notice(NoticeKind::Ok, format!("{}: {msg}", f.name));
                        }
                        ev(EventKind::Action, f.pid, &f.name, msg);
                    }
                    Err(e) => {
                        crate::diag::warn(&format!(
                            "Rule on {} (pid {}) failed: {e}",
                            f.name, f.pid
                        ));
                        ev(
                            EventKind::Action,
                            f.pid,
                            &f.name,
                            format!("Rule failed: {e}"),
                        );
                    }
                }
            }
        }

        // ── Load alerts ──
        if self.cfg.alerts_enabled {
            let sustain = Duration::from_secs(self.cfg.alert_secs);
            let top_cpu = s
                .procs
                .iter()
                .max_by(|a, b| a.cpu.total_cmp(&b.cpu))
                .map(|p| format!("{} ({:.0}%)", p.name, p.cpu))
                .unwrap_or_default();
            let top_mem = s
                .procs
                .iter()
                .max_by_key(|p| p.mem)
                .map(|p| format!("{} ({} MB)", p.name, p.mem / 1024 / 1024))
                .unwrap_or_default();
            if step(
                s.cpu_total,
                self.cfg.alert_cpu,
                sustain,
                &mut self.cpu_since,
                &mut self.cpu_alerted,
            ) {
                let msg = format!("CPU above {:.0}% — top: {top_cpu}", self.cfg.alert_cpu);
                ev(EventKind::Alert, 0, "System", msg.clone());
                self.notice(NoticeKind::Alert, format!("⚠ {msg}"));
            }
            if step(
                s.mem_pct(),
                self.cfg.alert_mem,
                sustain,
                &mut self.mem_since,
                &mut self.mem_alerted,
            ) {
                let msg = format!("Memory above {:.0}% — top: {top_mem}", self.cfg.alert_mem);
                ev(EventKind::Alert, 0, "System", msg.clone());
                self.notice(NoticeKind::Alert, format!("⚠ {msg}"));
            }
        }
    }
}

/// Returns true once when `value` has stayed above `threshold` for `sustain`.
/// Re-arms after the value drops 5 points below the threshold.
pub fn step(
    value: f32,
    threshold: f32,
    sustain: Duration,
    since: &mut Option<Instant>,
    alerted: &mut bool,
) -> bool {
    if value >= threshold {
        let start = *since.get_or_insert_with(Instant::now);
        if !*alerted && start.elapsed() >= sustain {
            *alerted = true;
            return true;
        }
    } else if value < threshold - 5.0 {
        *since = None;
        *alerted = false;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alert_fires_once_and_rearms() {
        let (mut since, mut alerted) = (None, false);
        assert!(step(95.0, 90.0, Duration::ZERO, &mut since, &mut alerted));
        assert!(!step(95.0, 90.0, Duration::ZERO, &mut since, &mut alerted));
        assert!(!step(88.0, 90.0, Duration::ZERO, &mut since, &mut alerted)); // within hysteresis
        assert!(!step(80.0, 90.0, Duration::ZERO, &mut since, &mut alerted)); // re-armed
        assert!(step(92.0, 90.0, Duration::ZERO, &mut since, &mut alerted));
    }
}
