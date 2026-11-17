//! Background sampling engine.
//!
//! A dedicated thread refreshes `sysinfo` on a configurable interval, turns the
//! raw data into a plain `Snapshot`, maintains rolling history buffers for the
//! graphs and records process start/exit events. The UI thread only ever reads
//! the finished snapshot, so the window stays smooth even on busy machines.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::longterm;
use crate::native::{self, GpuAdapter, NetStatus};
use sysinfo::{
    Components, CpuRefreshKind, Disks, MemoryRefreshKind, Networks, ProcessRefreshKind,
    ProcessesToUpdate, RefreshKind, System, UpdateKind, Users,
};

/// Maximum samples kept for system-wide graphs (15 minutes at 1 s).
pub const HISTORY_CAP: usize = 900;
/// Samples kept per process for the details panel sparklines.
pub const PROC_HISTORY_CAP: usize = 120;
/// Maximum entries in the activity log.
pub const LOG_CAP: usize = 2000;

/// Shared, immutable text. Process names, paths and command lines don't change
/// while a process runs, so each snapshot shares them instead of re-copying.
pub type Text = Arc<str>;

#[derive(Clone, Debug, Default)]
pub struct ProcInfo {
    pub pid: u32,
    pub parent: Option<u32>,
    pub name: Text,
    pub exe: Text,
    pub cmd: Text,
    pub user: Text,
    /// CPU usage normalised to the whole machine (0–100 %).
    pub cpu: f32,
    pub mem: u64,
    pub vmem: u64,
    /// Bytes per second.
    pub disk_read: u64,
    pub disk_write: u64,
    pub start_time: u64,
    pub run_time: u64,
    pub status: &'static str,
    /// Launch arguments (first item is the program) — used by "Restart".
    pub args: Arc<[String]>,
    pub cwd: Text,
    /// GPU utilisation of the busiest engine (0–100 %).
    pub gpu: f32,
    /// e.g. "GPU 0 - 3D".
    pub gpu_engine: Text,
    pub gpu_mem: u64,
    pub gpu_shared: u64,
    /// Network bytes per second (Windows, needs administrator rights).
    pub net_rx: u64,
    pub net_tx: u64,
}

#[derive(Clone, Debug, Default)]
pub struct NetIf {
    pub name: String,
    pub rx_rate: u64,
    pub tx_rate: u64,
    pub total_rx: u64,
    pub total_tx: u64,
    pub ips: Vec<String>,
    pub mac: String,
}

#[derive(Clone, Debug, Default)]
pub struct DiskInfo {
    pub name: String,
    pub mount: String,
    pub fs: String,
    pub kind: String,
    pub total: u64,
    pub available: u64,
    pub removable: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub procs: Vec<ProcInfo>,
    pub cpu_total: f32,
    pub per_core: Vec<f32>,
    pub cpu_brand: String,
    pub cpu_freq_mhz: u64,
    pub logical_cores: usize,
    pub physical_cores: Option<usize>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub mem_available: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub networks: Vec<NetIf>,
    pub net_rx_rate: u64,
    pub net_tx_rate: u64,
    pub disks: Vec<DiskInfo>,
    pub disk_read_rate: u64,
    pub disk_write_rate: u64,
    pub temps: Vec<(String, f32, Option<f32>)>,
    pub uptime: u64,
    pub boot_time: u64,
    pub os_name: String,
    pub host_name: String,
    pub kernel: String,
    pub arch: String,
    pub gpus: Vec<GpuAdapter>,
    pub net_status: Option<NetStatus>,
    /// Time the sampler spent on the previous refresh (ms, smoothed).
    pub sample_cost_ms: f32,
    pub seq: u64,
}

impl Snapshot {
    pub fn mem_pct(&self) -> f32 {
        pct(self.mem_used, self.mem_total)
    }
    pub fn swap_pct(&self) -> f32 {
        pct(self.swap_used, self.swap_total)
    }
    /// Busiest GPU's utilisation.
    pub fn gpu_util(&self) -> f32 {
        self.gpus.iter().map(|g| g.util).fold(0.0, f32::max)
    }
}

pub fn pct(a: u64, b: u64) -> f32 {
    if b == 0 {
        0.0
    } else {
        (a as f64 / b as f64 * 100.0) as f32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Started,
    Exited,
    Alert,
    Action,
}

#[derive(Clone, Debug)]
pub struct LogEvent {
    pub time: chrono::DateTime<chrono::Local>,
    pub kind: EventKind,
    pub pid: u32,
    pub name: String,
    pub detail: String,
}

/// Rolling histories used by the graphs.
#[derive(Default)]
pub struct History {
    pub cpu: VecDeque<f32>,
    pub mem: VecDeque<f32>,
    pub swap: VecDeque<f32>,
    pub per_core: Vec<VecDeque<f32>>,
    pub net_rx: VecDeque<f32>,
    pub net_tx: VecDeque<f32>,
    pub disk_r: VecDeque<f32>,
    pub disk_w: VecDeque<f32>,
    pub per_if: HashMap<String, (VecDeque<f32>, VecDeque<f32>)>,
    /// pid -> (start time, samples of (cpu %, memory bytes, gpu %)).
    /// The start time guards against a recycled PID inheriting old history.
    pub procs: HashMap<u32, (u64, VecDeque<(f32, f32, f32)>)>,
    pub gpus: Vec<GpuHistory>,
}

/// Per-adapter GPU history.
#[derive(Default)]
pub struct GpuHistory {
    pub util: VecDeque<f32>,
    pub engines: HashMap<String, VecDeque<f32>>,
    pub ded: VecDeque<f32>,
    pub shared: VecDeque<f32>,
    pub temp: VecDeque<f32>,
    pub power: VecDeque<f32>,
    pub core_mhz: VecDeque<f32>,
    pub fan: VecDeque<f32>,
}

pub fn push_capped<T>(q: &mut VecDeque<T>, v: T, cap: usize) {
    q.push_back(v);
    while q.len() > cap {
        q.pop_front();
    }
}

impl History {
    fn record(&mut self, s: &Snapshot) {
        push_capped(&mut self.cpu, s.cpu_total, HISTORY_CAP);
        push_capped(&mut self.mem, s.mem_pct(), HISTORY_CAP);
        push_capped(&mut self.swap, s.swap_pct(), HISTORY_CAP);
        if self.per_core.len() != s.per_core.len() {
            self.per_core = vec![VecDeque::new(); s.per_core.len()];
        }
        for (q, v) in self.per_core.iter_mut().zip(&s.per_core) {
            push_capped(q, *v, HISTORY_CAP);
        }
        push_capped(&mut self.net_rx, s.net_rx_rate as f32, HISTORY_CAP);
        push_capped(&mut self.net_tx, s.net_tx_rate as f32, HISTORY_CAP);
        push_capped(&mut self.disk_r, s.disk_read_rate as f32, HISTORY_CAP);
        push_capped(&mut self.disk_w, s.disk_write_rate as f32, HISTORY_CAP);
        for n in &s.networks {
            let e = self.per_if.entry(n.name.clone()).or_default();
            push_capped(&mut e.0, n.rx_rate as f32, HISTORY_CAP);
            push_capped(&mut e.1, n.tx_rate as f32, HISTORY_CAP);
        }
        let alive: HashSet<u32> = s.procs.iter().map(|p| p.pid).collect();
        self.procs.retain(|pid, _| alive.contains(pid));
        for p in &s.procs {
            let e = self
                .procs
                .entry(p.pid)
                .or_insert_with(|| (p.start_time, VecDeque::new()));
            if e.0 != p.start_time {
                *e = (p.start_time, VecDeque::new());
            }
            push_capped(&mut e.1, (p.cpu, p.mem as f32, p.gpu), PROC_HISTORY_CAP);
        }
        if self.gpus.len() != s.gpus.len() {
            self.gpus = (0..s.gpus.len()).map(|_| GpuHistory::default()).collect();
        }
        for (h, g) in self.gpus.iter_mut().zip(&s.gpus) {
            push_capped(&mut h.util, g.util, HISTORY_CAP);
            for e in &g.engines {
                push_capped(
                    h.engines.entry(e.name.clone()).or_default(),
                    e.util,
                    HISTORY_CAP,
                );
            }
            push_capped(&mut h.ded, g.ded_used as f32, HISTORY_CAP);
            push_capped(&mut h.shared, g.shared_used as f32, HISTORY_CAP);
            push_capped(&mut h.temp, g.temp_c.unwrap_or(0.0), HISTORY_CAP);
            push_capped(&mut h.power, g.power_pct.unwrap_or(0.0), HISTORY_CAP);
            push_capped(&mut h.core_mhz, g.core_mhz.unwrap_or(0.0), HISTORY_CAP);
            push_capped(&mut h.fan, g.fan_rpm.unwrap_or(0) as f32, HISTORY_CAP);
        }
    }
}

/// State shared between the sampler thread and the UI.
pub struct Shared {
    pub snap: Arc<Snapshot>,
    pub history: History,
    pub log: VecDeque<Arc<LogEvent>>,
    pub longterm: longterm::Store,
    pub leaks: crate::insights::LeakTracker,
    pub leak_suspects: Vec<crate::insights::LeakSuspect>,
    pub impact: crate::insights::ImpactTracker,
}

/// Lets the UI wake the sampler early (refresh now / interval changed)
/// without the sampler having to poll.
#[derive(Default)]
pub struct Wake {
    flag: Mutex<bool>,
    cv: Condvar,
}

impl Wake {
    pub fn notify(&self) {
        if let Ok(mut f) = self.flag.lock() {
            *f = true;
            self.cv.notify_one();
        }
    }
    /// Sleep until `deadline` or until notified. Returns true if notified.
    fn wait_until(&self, deadline: Instant) -> bool {
        let Ok(mut f) = self.flag.lock() else {
            return false;
        };
        loop {
            if *f {
                *f = false;
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            match self.cv.wait_timeout(f, deadline - now) {
                Ok((g, _)) => f = g,
                Err(_) => return false,
            }
        }
    }
}

pub struct Monitor {
    pub shared: Arc<Mutex<Shared>>,
    pub automation: Arc<Mutex<crate::automation::Automation>>,
    pub interval_ms: Arc<AtomicU64>,
    /// Freezes the *view* only; sampling, rules, alerts and history keep running.
    pub paused: Arc<AtomicBool>,
    pub wake: Arc<Wake>,
}

impl Monitor {
    pub fn start(interval_ms: u64, repaint: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            snap: Arc::new(Snapshot::default()),
            history: History::default(),
            log: VecDeque::new(),
            longterm: longterm::Store::load(),
            leaks: Default::default(),
            leak_suspects: Vec::new(),
            impact: crate::insights::ImpactTracker::load(),
        }));
        let interval = Arc::new(AtomicU64::new(interval_ms.max(250)));
        let paused = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(Wake::default());

        let automation = Arc::new(Mutex::new(crate::automation::Automation::default()));
        let au = automation.clone();
        let (sh, iv, wk) = (shared.clone(), interval.clone(), wake.clone());
        thread::Builder::new()
            .name("sampler".into())
            .spawn(move || sampler_loop(sh, au, iv, wk, repaint))
            .expect("failed to spawn sampler thread");

        Monitor {
            shared,
            automation,
            interval_ms: interval,
            paused,
            wake,
        }
    }

    /// Take a new sample right away.
    pub fn refresh_now(&self) {
        self.wake.notify();
    }

    /// Change the sampling interval (wakes the sampler so it applies at once).
    pub fn set_interval(&self, ms: u64) {
        let ms = ms.max(250);
        if self.interval_ms.swap(ms, Ordering::Relaxed) != ms {
            self.wake.notify();
        }
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.shared.lock().unwrap().snap.clone()
    }

    pub fn log_action(&self, pid: u32, name: &str, detail: impl Into<String>) {
        self.push_event(EventKind::Action, pid, name, detail);
    }

    pub fn push_event(&self, kind: EventKind, pid: u32, name: &str, detail: impl Into<String>) {
        let mut g = self.shared.lock().unwrap();
        push_capped(
            &mut g.log,
            Arc::new(LogEvent {
                time: chrono::Local::now(),
                kind,
                pid,
                name: name.to_string(),
                detail: detail.into(),
            }),
            LOG_CAP,
        );
    }
}

/// Per-process data that never changes while the process runs.
struct Static {
    name: Text,
    exe: Text,
    cmd: Text,
    cwd: Text,
    user: Text,
    args: Arc<[String]>,
}

fn status_str(s: sysinfo::ProcessStatus) -> &'static str {
    use sysinfo::ProcessStatus as S;
    match s {
        S::Run => "Run",
        S::Sleep => "Sleep",
        S::Stop => "Stop",
        S::Idle => "Idle",
        S::Zombie => "Zombie",
        S::Dead => "Dead",
        S::Tracing => "Tracing",
        S::Wakekill => "Wakekill",
        S::Waking => "Waking",
        S::Parked => "Parked",
        S::LockBlocked => "LockBlocked",
        S::UninterruptibleDiskSleep => "DiskSleep",
        S::Unknown(_) => "Unknown",
    }
}

fn sampler_loop(
    shared: Arc<Mutex<Shared>>,
    automation: Arc<Mutex<crate::automation::Automation>>,
    interval: Arc<AtomicU64>,
    wake: Arc<Wake>,
    repaint: impl Fn(),
) {
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything()),
    );
    let mut networks = Networks::new_with_refreshed_list();
    let mut disks = Disks::new_with_refreshed_list();
    // Temperature sensors are slow to query on Windows (WMI) and usually absent;
    // only keep polling them if the first scan found any.
    let mut components = Components::new_with_refreshed_list();
    let has_sensors = !components.list().is_empty();
    let mut users = Users::new_with_refreshed_list();

    let proc_kind = ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_disk_usage()
        .with_exe(UpdateKind::OnlyIfNotSet)
        .with_cmd(UpdateKind::OnlyIfNotSet)
        .with_user(UpdateKind::OnlyIfNotSet)
        .with_cwd(UpdateKind::OnlyIfNotSet);

    let mut gpu = native::GpuSampler::new();
    let net = native::NetMonitor::start();
    let net_status = Some(match &net {
        Ok(_) => NetStatus::Running,
        Err(e) => e.clone(),
    });
    crate::diag::info(&format!("Per-app network monitoring: {net_status:?}"));

    // Facts that don't change while we run.
    let os_name = System::long_os_version().unwrap_or_else(|| "Unknown OS".into());
    let host_name = System::host_name().unwrap_or_default();
    let kernel = System::kernel_version().unwrap_or_default();
    let arch = System::cpu_arch();
    let boot_time = System::boot_time();

    let mut last_longterm = 0i64;
    let mut cost_ema = 0.0f32;
    let mut last_save = Instant::now();

    // Prime CPU counters so the first real sample is meaningful.
    sys.refresh_cpu_usage();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, proc_kind);
    thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(250)));

    let mut last = Instant::now();
    let mut seq: u64 = 0;
    // (pid, start time) -> unchanging details, shared between snapshots.
    let mut statics: HashMap<(u32, u64), Arc<Static>> = HashMap::new();
    let mut prev: Arc<Snapshot> = Arc::new(Snapshot::default());
    let mut slow_tick = 0u32;

    loop {
        let tick_start = Instant::now();
        let dt = tick_start.duration_since(last).as_secs_f64().max(0.001);
        last = tick_start;

        sys.refresh_cpu_all();
        sys.refresh_memory();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, proc_kind);
        networks.refresh(true);

        // Slow-changing sources are refreshed every ~10 samples.
        if slow_tick.is_multiple_of(10) {
            disks.refresh(true);
            if has_sensors {
                components.refresh(true);
            }
        }
        if slow_tick.is_multiple_of(60) {
            users.refresh();
        }
        slow_tick = slow_tick.wrapping_add(1);

        let prev_pids: HashMap<u32, u64> =
            prev.procs.iter().map(|p| (p.pid, p.start_time)).collect();
        let ncpu = sys.cpus().len().max(1) as f32;
        let mut procs = Vec::with_capacity(sys.processes().len());
        let (mut dr, mut dw) = (0u64, 0u64);
        for (pid, p) in sys.processes() {
            // Skip Linux threads so the list matches what Windows shows.
            if p.thread_kind() == Some(sysinfo::ThreadKind::Userland) {
                continue;
            }
            let pid = pid.as_u32();
            let start_time = p.start_time();
            let du = p.disk_usage();
            // A process's first reading covers everything since it started,
            // not the last interval — skip it to avoid a fake spike.
            let fresh = prev_pids.get(&pid) != Some(&start_time);
            let (r, w) = if fresh {
                (0, 0)
            } else {
                (
                    (du.read_bytes as f64 / dt) as u64,
                    (du.written_bytes as f64 / dt) as u64,
                )
            };
            dr += r;
            dw += w;
            let st = statics
                .entry((pid, start_time))
                .or_insert_with(|| {
                    let args: Vec<String> = p
                        .cmd()
                        .iter()
                        .map(|s| s.to_string_lossy().into_owned())
                        .collect();
                    Arc::new(Static {
                        name: p.name().to_string_lossy().into(),
                        exe: p
                            .exe()
                            .map(|e| e.display().to_string())
                            .unwrap_or_default()
                            .into(),
                        cmd: args.join(" ").into(),
                        cwd: p
                            .cwd()
                            .map(|c| c.display().to_string())
                            .unwrap_or_default()
                            .into(),
                        user: p
                            .user_id()
                            .and_then(|u| users.get_user_by_id(u))
                            .map(|u| u.name())
                            .unwrap_or_default()
                            .into(),
                        args: args.into(),
                    })
                })
                .clone();
            procs.push(ProcInfo {
                pid,
                parent: p.parent().map(|x| x.as_u32()),
                name: st.name.clone(),
                exe: st.exe.clone(),
                cmd: st.cmd.clone(),
                cwd: st.cwd.clone(),
                user: st.user.clone(),
                args: st.args.clone(),
                cpu: (p.cpu_usage() / ncpu).clamp(0.0, 100.0),
                mem: p.memory(),
                vmem: p.virtual_memory(),
                disk_read: r,
                disk_write: w,
                start_time,
                run_time: p.run_time(),
                status: status_str(p.status()),
                ..Default::default()
            });
        }
        // Forget details of processes that have exited.
        if statics.len() > procs.len() + 64 {
            let alive: HashSet<(u32, u64)> = procs.iter().map(|p| (p.pid, p.start_time)).collect();
            statics.retain(|k, _| alive.contains(k));
        }

        // GPU: per-process engines and memory. (The pid list only feeds demo data.)
        let pid_names: Vec<(u32, &str)> = if cfg!(windows) {
            Vec::new()
        } else {
            let mut v: Vec<&ProcInfo> = procs.iter().collect();
            v.sort_by(|a, b| b.mem.cmp(&a.mem));
            v.iter().map(|p| (p.pid, &*p.name)).collect()
        };
        let gs = gpu.sample(&pid_names);
        drop(pid_names);
        if !gs.procs.is_empty() {
            for p in procs.iter_mut() {
                if let Some(g) = gs.procs.get(&p.pid) {
                    p.gpu = g.util;
                    p.gpu_engine = g.engine.as_str().into();
                    p.gpu_mem = g.dedicated;
                    p.gpu_shared = g.shared;
                }
            }
        }

        // Per-app network throughput.
        if let Ok(n) = &net {
            let bytes = n.take();
            if !bytes.is_empty() {
                let index: HashMap<u32, usize> =
                    procs.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();
                let mut by_mem: Vec<usize> = Vec::new();
                for (pid, (rx, tx)) in bytes {
                    let mut idx = index.get(&pid).copied();
                    // Demo data is keyed by rank from u32::MAX downwards.
                    if idx.is_none() && pid >= u32::MAX - 16 {
                        if by_mem.is_empty() {
                            by_mem = (0..procs.len()).collect();
                            by_mem.sort_by(|a, b| procs[*b].mem.cmp(&procs[*a].mem));
                        }
                        idx = by_mem.get((u32::MAX - pid) as usize).copied();
                    }
                    if let Some(i) = idx {
                        procs[i].net_rx += (rx as f64 / dt) as u64;
                        procs[i].net_tx += (tx as f64 / dt) as u64;
                    }
                }
            }
        }

        let mut nets: Vec<NetIf> = networks
            .list()
            .iter()
            .map(|(name, d)| NetIf {
                name: name.clone(),
                rx_rate: (d.received() as f64 / dt) as u64,
                tx_rate: (d.transmitted() as f64 / dt) as u64,
                total_rx: d.total_received(),
                total_tx: d.total_transmitted(),
                ips: d
                    .ip_networks()
                    .iter()
                    .map(|ip| ip.addr.to_string())
                    .collect(),
                mac: d.mac_address().to_string(),
            })
            .collect();
        nets.sort_by(|a, b| (b.total_rx + b.total_tx).cmp(&(a.total_rx + a.total_tx)));

        let cpus = sys.cpus();
        let snap = Snapshot {
            cpu_total: sys.global_cpu_usage(),
            per_core: cpus.iter().map(|c| c.cpu_usage()).collect(),
            cpu_brand: if prev.cpu_brand.is_empty() {
                cpus.first()
                    .map(|c| c.brand().trim().to_string())
                    .unwrap_or_default()
            } else {
                prev.cpu_brand.clone()
            },
            cpu_freq_mhz: cpus.first().map(|c| c.frequency()).unwrap_or(0),
            logical_cores: cpus.len(),
            physical_cores: if seq == 0 {
                sys.physical_core_count()
            } else {
                prev.physical_cores
            },
            mem_used: sys.used_memory(),
            mem_total: sys.total_memory(),
            mem_available: sys.available_memory(),
            swap_used: sys.used_swap(),
            swap_total: sys.total_swap(),
            net_rx_rate: nets.iter().map(|n| n.rx_rate).sum(),
            net_tx_rate: nets.iter().map(|n| n.tx_rate).sum(),
            networks: nets,
            disks: if slow_tick % 10 == 1 || seq == 0 {
                disks
                    .list()
                    .iter()
                    .map(|d| DiskInfo {
                        name: d.name().to_string_lossy().into_owned(),
                        mount: d.mount_point().display().to_string(),
                        fs: d.file_system().to_string_lossy().into_owned(),
                        kind: format!("{:?}", d.kind()),
                        total: d.total_space(),
                        available: d.available_space(),
                        removable: d.is_removable(),
                    })
                    .collect()
            } else {
                prev.disks.clone()
            },
            disk_read_rate: dr,
            disk_write_rate: dw,
            temps: if has_sensors {
                components
                    .list()
                    .iter()
                    .filter_map(|c| {
                        c.temperature()
                            .map(|t| (c.label().to_string(), t, c.critical()))
                    })
                    .collect()
            } else {
                Vec::new()
            },
            uptime: System::uptime(),
            boot_time,
            os_name: os_name.clone(),
            host_name: host_name.clone(),
            kernel: kernel.clone(),
            arch: arch.clone(),
            gpus: gs.adapters,
            net_status: net_status.clone(),
            sample_cost_ms: cost_ema,
            seq,
            procs,
        };

        // Process start / exit tracking (compared with the previous snapshot).
        let mut events = Vec::new();
        if seq > 0 {
            let now_local = chrono::Local::now();
            let current: HashMap<u32, u64> =
                snap.procs.iter().map(|p| (p.pid, p.start_time)).collect();
            for p in &snap.procs {
                if prev_pids.get(&p.pid) != Some(&p.start_time) {
                    events.push(LogEvent {
                        time: now_local,
                        kind: EventKind::Started,
                        pid: p.pid,
                        name: p.name.to_string(),
                        detail: if p.cmd.is_empty() {
                            p.exe.to_string()
                        } else {
                            p.cmd.to_string()
                        },
                    });
                }
            }
            for p in &prev.procs {
                if current.get(&p.pid) != Some(&p.start_time) {
                    events.push(LogEvent {
                        time: now_local,
                        kind: EventKind::Exited,
                        pid: p.pid,
                        name: p.name.to_string(),
                        detail: String::new(),
                    });
                }
            }
        }
        seq += 1;

        // Rules and alerts run here so they keep working while the window is hidden.
        let mut auto_events = VecDeque::new();
        if let Ok(mut a) = automation.lock() {
            a.on_snapshot(&snap, &mut auto_events);
        }
        events.extend(auto_events);

        let snap = Arc::new(snap);
        let unix = longterm::now();
        let save_due = last_save.elapsed().as_secs() >= 60;
        let (pending_history, impact_json) = {
            let mut g = shared.lock().unwrap();
            g.history.record(&snap);
            g.leaks.update(unix, &snap.procs);
            if seq % 60 == 1 {
                g.leak_suspects = g.leaks.suspects();
            }
            g.impact
                .update(&snap.procs, ncpu, dt, boot_time, unix as u64);
            if unix - last_longterm >= longterm::STEP_SECS {
                last_longterm = unix;
                let top = |f: &dyn Fn(&ProcInfo) -> f32| {
                    snap.procs
                        .iter()
                        .max_by(|a, b| f(a).total_cmp(&f(b)))
                        .filter(|p| f(p) > 0.0)
                        .map(|p| p.name.clone())
                        .unwrap_or_default()
                };
                g.longterm.push(longterm::Point {
                    t: unix,
                    cpu: snap.cpu_total,
                    mem: snap.mem_pct(),
                    gpu: snap.gpu_util(),
                    net: (snap.net_rx_rate + snap.net_tx_rate) as f32,
                    disk: (snap.disk_read_rate + snap.disk_write_rate) as f32,
                    top_cpu: top(&|p| p.cpu),
                    top_mem: top(&|p| p.mem as f32),
                    top_gpu: top(&|p| p.gpu),
                });
            }
            for e in events {
                push_capped(&mut g.log, Arc::new(e), LOG_CAP);
            }
            g.snap = snap.clone();
            // Grab what needs saving; the file writes happen after the lock is released.
            if save_due {
                (g.longterm.take_unsaved(), g.impact.to_json())
            } else {
                (None, None)
            }
        };
        if save_due {
            last_save = Instant::now();
            if let Some(w) = pending_history {
                w.write();
            }
            if let Some(j) = impact_json {
                crate::insights::ImpactTracker::write(&j);
            }
        }
        prev = snap;

        let took = tick_start.elapsed();
        let ms = took.as_secs_f32() * 1000.0;
        cost_ema = if cost_ema == 0.0 {
            ms
        } else {
            cost_ema * 0.9 + ms * 0.1
        };
        if took > Duration::from_secs(3) {
            crate::diag::warn(&format!(
                "Sampler: one refresh took {:.1}s",
                took.as_secs_f32()
            ));
        }
        repaint();

        // Sleep until the next tick (measured from the start of this one, so the
        // interval doesn't drift), or until the UI asks for a refresh.
        let deadline =
            tick_start + Duration::from_millis(interval.load(Ordering::Relaxed).max(250));
        wake.wait_until(deadline);
    }
}

impl Monitor {
    /// Flush long-term history to disk (called when the app closes).
    pub fn save_history(&self) {
        let (history, impact) = match self.shared.lock() {
            Ok(mut g) => (g.longterm.take_unsaved(), g.impact.to_json()),
            Err(_) => return,
        };
        if let Some(w) = history {
            w.write();
        }
        if let Some(j) = impact {
            crate::insights::ImpactTracker::write(&j);
        }
    }
}
