//! Automation rules (Process Lasso-style).
//!
//! Each rule matches process names with `*` / `?` wildcards and applies one
//! action. Evaluation is pure: it takes a snapshot and returns the effects to
//! carry out, so the logic is unit-tested without touching the OS.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::monitor::ProcInfo;
use crate::native::wildcard_match;
use crate::platform::Priority;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RuleAction {
    /// Set priority whenever the process starts.
    Priority(Priority),
    /// Pin to these CPUs whenever the process starts.
    Affinity(u64),
    /// End the process whenever it starts.
    Block,
    /// Lower priority to Below normal once CPU stays above `over` % for `secs`.
    Tame { over: f32, secs: u64 },
    /// Notify once CPU stays above `over` % for `secs`.
    Notify { over: f32, secs: u64 },
    /// Notify once the process uses more than `mb` MB of memory.
    MemoryLimit { mb: u64 },
}

impl RuleAction {
    pub fn describe(&self) -> String {
        match self {
            RuleAction::Priority(p) => format!("Set priority to {}", p.label()),
            RuleAction::Affinity(m) => format!("Limit to {} CPU(s) (mask 0x{m:X})", m.count_ones()),
            RuleAction::Block => "Block — end it whenever it starts".into(),
            RuleAction::Tame { over, secs } => {
                format!("If CPU > {over:.0}% for {secs}s, lower its priority")
            }
            RuleAction::Notify { over, secs } => format!("Notify if CPU > {over:.0}% for {secs}s"),
            RuleAction::MemoryLimit { mb } => format!("Notify if memory > {mb} MB"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub enabled: bool,
    pub pattern: String,
    pub action: RuleAction,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Priority(Priority),
    Affinity(u64),
    Kill,
    Tame,
    Notify(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fired {
    pub pid: u32,
    pub name: String,
    pub effect: Effect,
    pub rule: String,
}

/// Remembers what has already been applied so rules fire once per process.
#[derive(Default)]
pub struct RuleState {
    /// (pid, start time) pairs already handled by start-time rules.
    seen: HashSet<(u32, u64)>,
    since: HashMap<(u32, usize), Instant>,
    fired: HashSet<(u32, usize)>,
}

impl RuleState {
    /// Forget everything (e.g. after the rule list changed) so rules re-apply.
    pub fn reset(&mut self) {
        *self = RuleState::default();
    }
}

pub fn evaluate(
    state: &mut RuleState,
    rules: &[Rule],
    procs: &[ProcInfo],
    now: Instant,
) -> Vec<Fired> {
    let mut out = Vec::new();
    let me = std::process::id();
    let alive: HashSet<(u32, u64)> = procs.iter().map(|p| (p.pid, p.start_time)).collect();
    state.seen.retain(|k| alive.contains(k));
    let alive_pids: HashSet<u32> = procs.iter().map(|p| p.pid).collect();
    state.since.retain(|k, _| alive_pids.contains(&k.0));
    state.fired.retain(|k| alive_pids.contains(&k.0));

    for p in procs {
        if p.pid == me || p.pid <= 4 {
            continue;
        }
        let first_time = state.seen.insert((p.pid, p.start_time));
        for (ri, r) in rules.iter().enumerate() {
            if !r.enabled
                || r.pattern.trim().is_empty()
                || !wildcard_match(r.pattern.trim(), &p.name)
            {
                continue;
            }
            let fire = |effect: Effect| Fired {
                pid: p.pid,
                name: p.name.to_string(),
                effect,
                rule: r.action.describe(),
            };
            match &r.action {
                RuleAction::Priority(pr) if first_time => out.push(fire(Effect::Priority(*pr))),
                RuleAction::Affinity(m) if first_time => out.push(fire(Effect::Affinity(*m))),
                RuleAction::Block if first_time => out.push(fire(Effect::Kill)),
                RuleAction::Tame { over, secs } | RuleAction::Notify { over, secs } => {
                    let key = (p.pid, ri);
                    if p.cpu >= *over {
                        let start = *state.since.entry(key).or_insert(now);
                        if now.duration_since(start) >= Duration::from_secs(*secs)
                            && state.fired.insert(key)
                        {
                            out.push(fire(if matches!(r.action, RuleAction::Tame { .. }) {
                                Effect::Tame
                            } else {
                                Effect::Notify(format!(
                                    "{} has used over {over:.0}% CPU for {secs}s",
                                    p.name
                                ))
                            }));
                        }
                    } else {
                        state.since.remove(&key);
                        if p.cpu < over - 10.0 {
                            state.fired.remove(&key);
                        }
                    }
                }
                RuleAction::MemoryLimit { mb } => {
                    let key = (p.pid, ri);
                    if p.mem >= mb * 1024 * 1024 {
                        if state.fired.insert(key) {
                            out.push(fire(Effect::Notify(format!(
                                "{} is using {} MB of memory (limit {mb} MB)",
                                p.name,
                                p.mem / 1024 / 1024
                            ))));
                        }
                    } else {
                        state.fired.remove(&key);
                    }
                }
                _ => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, name: &str, cpu: f32) -> ProcInfo {
        ProcInfo {
            pid,
            name: name.into(),
            cpu,
            start_time: 100,
            ..Default::default()
        }
    }

    #[test]
    fn start_rules_fire_once() {
        let rules = vec![Rule {
            enabled: true,
            pattern: "game*.exe".into(),
            action: RuleAction::Priority(Priority::High),
        }];
        let mut st = RuleState::default();
        let t = Instant::now();
        let ps = vec![proc(100, "game.exe", 1.0), proc(101, "other.exe", 1.0)];
        let f = evaluate(&mut st, &rules, &ps, t);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].effect, Effect::Priority(Priority::High));
        assert!(evaluate(&mut st, &rules, &ps, t).is_empty());
    }

    #[test]
    fn tame_needs_sustained_load() {
        let rules = vec![Rule {
            enabled: true,
            pattern: "hog.exe".into(),
            action: RuleAction::Tame {
                over: 50.0,
                secs: 5,
            },
        }];
        let mut st = RuleState::default();
        let t0 = Instant::now();
        let hot = vec![proc(200, "hog.exe", 80.0)];
        assert!(evaluate(&mut st, &rules, &hot, t0).is_empty());
        assert!(evaluate(&mut st, &rules, &hot, t0 + Duration::from_secs(3)).is_empty());
        let f = evaluate(&mut st, &rules, &hot, t0 + Duration::from_secs(6));
        assert_eq!(f[0].effect, Effect::Tame);
        assert!(evaluate(&mut st, &rules, &hot, t0 + Duration::from_secs(9)).is_empty());
    }

    #[test]
    fn disabled_and_block() {
        let rules = vec![
            Rule {
                enabled: false,
                pattern: "*".into(),
                action: RuleAction::Block,
            },
            Rule {
                enabled: true,
                pattern: "bad?.exe".into(),
                action: RuleAction::Block,
            },
        ];
        let mut st = RuleState::default();
        let f = evaluate(
            &mut st,
            &rules,
            &[proc(300, "bad1.exe", 0.0), proc(301, "good.exe", 0.0)],
            Instant::now(),
        );
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].pid, 300);
        assert_eq!(f[0].effect, Effect::Kill);
    }

    #[test]
    fn memory_limit() {
        let rules = vec![Rule {
            enabled: true,
            pattern: "*".into(),
            action: RuleAction::MemoryLimit { mb: 100 },
        }];
        let mut st = RuleState::default();
        let mut p = proc(400, "big.exe", 0.0);
        p.mem = 200 * 1024 * 1024;
        assert_eq!(
            evaluate(&mut st, &rules, &[p.clone()], Instant::now()).len(),
            1
        );
        assert!(evaluate(&mut st, &rules, &[p], Instant::now()).is_empty());
    }
}
