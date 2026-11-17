//! Application state, shell layout (sidebar, header, status bar), background
//! jobs, dialogs, alerts, toasts and keyboard shortcuts.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, vec2, Align, Color32, Key, Layout, Margin, RichText, Stroke};
use serde::{Deserialize, Serialize};

use crate::monitor::{Monitor, Snapshot};
use crate::platform::{self, Conn, Service, StartupItem};
use crate::ui::theme::{self, Palette};
use crate::ui::widgets::{self, fmt_bytes, fmt_dur, fmt_rate};

// ───────────────────────────── Settings ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewMode {
    Flat,
    Tree,
    Grouped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Col {
    Name,
    Pid,
    Status,
    User,
    Cpu,
    Mem,
    Disk,
    Started,
    Path,
    Cmd,
    Gpu,
    GpuMem,
    Net,
    Publisher,
}

impl Col {
    pub const ALL: [Col; 14] = [
        Col::Name,
        Col::Pid,
        Col::Status,
        Col::User,
        Col::Cpu,
        Col::Mem,
        Col::Disk,
        Col::Gpu,
        Col::GpuMem,
        Col::Net,
        Col::Publisher,
        Col::Started,
        Col::Path,
        Col::Cmd,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Col::Name => "Name",
            Col::Pid => "PID",
            Col::Status => "Status",
            Col::User => "User",
            Col::Cpu => "CPU",
            Col::Mem => "Memory",
            Col::Disk => "Disk",
            Col::Started => "Started",
            Col::Path => "Path",
            Col::Cmd => "Command line",
            Col::Gpu => "GPU",
            Col::GpuMem => "GPU memory",
            Col::Net => "Network",
            Col::Publisher => "Publisher",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub dark: bool,
    pub accent: usize,
    pub refresh_ms: u64,
    pub graph_secs: u64,
    pub view_mode: ViewMode,
    pub columns: Vec<Col>,
    pub sort_col: Col,
    pub sort_desc: bool,
    pub heatmap: bool,
    pub confirm_kill: bool,
    pub compact_rows: bool,
    pub show_details: bool,
    pub always_on_top: bool,
    /// Show the live tray icon (Windows).
    pub tray_icon: bool,
    /// Closing the window hides it to the tray instead of quitting.
    pub close_to_tray: bool,
    pub alerts_enabled: bool,
    pub alert_cpu: f32,
    pub alert_mem: f32,
    pub alert_secs: u64,
    pub pinned: Vec<String>,
    pub only_mine: bool,
    pub log_process_events: bool,
    /// User chose "Run without installing" on the welcome screen.
    pub install_prompt_dismissed: bool,
    pub rules: Vec<crate::rules::Rule>,
    /// Columns were upgraded to include the GPU / network columns.
    pub columns_v2: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            dark: true,
            accent: 0,
            refresh_ms: 1000,
            graph_secs: 60,
            view_mode: ViewMode::Grouped,
            columns: vec![
                Col::Name,
                Col::Pid,
                Col::User,
                Col::Cpu,
                Col::Mem,
                Col::Disk,
                Col::Gpu,
                Col::Net,
            ],
            sort_col: Col::Cpu,
            sort_desc: true,
            heatmap: true,
            confirm_kill: true,
            compact_rows: false,
            show_details: true,
            always_on_top: false,
            tray_icon: true,
            close_to_tray: false,
            alerts_enabled: true,
            alert_cpu: 90.0,
            alert_mem: 90.0,
            alert_secs: 10,
            pinned: Vec::new(),
            only_mine: false,
            log_process_events: true,
            install_prompt_dismissed: false,
            rules: Vec::new(),
            // false so settings saved by older versions get the new columns once.
            columns_v2: false,
        }
    }
}

// ───────────────────────────── Pages ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Overview,
    Health,
    Repair,
    Processes,
    Performance,
    History,
    Network,
    Startup,
    Services,
    Rules,
    Tools,
    Activity,
    Settings,
}

impl Page {
    pub const ALL: [Page; 13] = [
        Page::Overview,
        Page::Health,
        Page::Repair,
        Page::Processes,
        Page::Performance,
        Page::History,
        Page::Network,
        Page::Startup,
        Page::Services,
        Page::Rules,
        Page::Tools,
        Page::Activity,
        Page::Settings,
    ];
    pub fn icon(self) -> &'static str {
        match self {
            Page::Overview => "🏠",
            Page::Health => "❤",
            Page::Repair => "🚑",
            Page::Processes => "☰",
            Page::Performance => "📈",
            Page::Network => "🌐",
            Page::Startup => "🚀",
            Page::Services => "⚙",
            Page::History => "📅",
            Page::Rules => "⚡",
            Page::Tools => "🔨",
            Page::Activity => "🕘",
            Page::Settings => "🔧",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Page::Overview => "Overview",
            Page::Health => "Health check",
            Page::Repair => "Fix Windows",
            Page::Processes => "Processes",
            Page::Performance => "Performance",
            Page::Network => "Connections",
            Page::Startup => "Startup apps",
            Page::Services => "Services",
            Page::History => "History",
            Page::Rules => "Automation",
            Page::Tools => "Tools",
            Page::Activity => "Activity log",
            Page::Settings => "Settings",
        }
    }
    pub fn subtitle(self) -> &'static str {
        match self {
            Page::Overview => "Your system at a glance",
            Page::Health => "Why is my PC slow? Plain answers and one-click fixes",
            Page::Repair => "Built-in Windows diagnostics and repairs, one click each",
            Page::Processes => "Everything running right now",
            Page::Performance => "Live and historical resource usage",
            Page::Network => "Which apps are talking to the internet",
            Page::Startup => "Control what launches when you sign in",
            Page::Services => "Background Windows services",
            Page::History => "The last 24 hours, and what caused each spike",
            Page::Rules => "Rules that run automatically as processes start or misbehave",
            Page::Tools => "Find a window's process, unlock files, quick fixes",
            Page::Activity => "Process launches, exits, alerts and actions",
            Page::Settings => "Make it yours",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PerfSel {
    Cpu,
    Memory,
    Disk,
    Net(String),
    Gpu(usize),
}

// ───────────────────────────── Jobs & dialogs ─────────────────────────────

#[derive(Clone, Debug)]
pub enum Follow {
    None,
    Startup,
    Services,
    Sched,
    Priority(u32),
    AffinityLoaded(u32),
    PriorityLoaded(u32),
    /// SHA-256 computed for this pid (result is the hash).
    Hash(u32),
    /// Suspend/resume finished; refresh the suspended set.
    Suspend(u32, bool),
}

pub struct JobResult {
    pub res: Result<String, String>,
    pub follow: Follow,
    pub pid: u32,
    pub name: String,
    pub quiet: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ToastKind {
    Ok,
    Err,
    Info,
}

pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub at: Instant,
}

#[derive(Clone)]
pub enum ConfirmAction {
    Kill { pid: u32, name: String, tree: bool },
    KillAll { name: String, pids: Vec<u32> },
    DeleteStartup(StartupItem),
    StopService(String),
    Uninstall,
    KillThread { pid: u32, tid: u32 },
}

pub struct Confirm {
    pub title: String,
    pub body: String,
    pub warn: Option<String>,
    pub action: ConfirmAction,
}

pub struct AffinityDlg {
    pub pid: u32,
    pub name: String,
    pub mask: Option<u64>,
}

#[derive(Default)]
pub struct RunDlg {
    pub cmd: String,
    pub admin: bool,
    pub focus: bool,
}

pub struct InstallDlg {
    pub desktop: bool,
    pub login: bool,
    pub busy: bool,
}

/// The "new rule" form on the Automation page.
pub struct RuleDraft {
    pub pattern: String,
    pub kind: usize,
    pub priority: platform::Priority,
    pub over: f32,
    pub secs: u64,
    pub mb: u64,
    pub mask: u64,
}

impl Default for RuleDraft {
    fn default() -> Self {
        RuleDraft {
            pattern: String::new(),
            kind: 0,
            priority: platform::Priority::High,
            over: 80.0,
            secs: 30,
            mb: 2048,
            mask: 0b11,
        }
    }
}

#[derive(Default)]
pub struct AddStartupDlg {
    pub name: String,
    pub cmd: String,
}

/// A list loaded off the UI thread.
pub struct AsyncList<T> {
    pub data: Vec<T>,
    pub loading: bool,
    pub error: Option<String>,
    pub loaded_at: Option<Instant>,
    rx: Option<Receiver<Result<Vec<T>, String>>>,
}

impl<T> Default for AsyncList<T> {
    fn default() -> Self {
        AsyncList {
            data: Vec::new(),
            loading: false,
            error: None,
            loaded_at: None,
            rx: None,
        }
    }
}

impl<T: Send + 'static> AsyncList<T> {
    pub fn load(
        &mut self,
        ctx: &egui::Context,
        f: impl FnOnce() -> Result<Vec<T>, String> + Send + 'static,
    ) {
        if self.loading {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
        self.rx = Some(rx);
        self.loading = true;
    }
    pub fn poll(&mut self) {
        if let Some(rx) = &self.rx {
            if let Ok(r) = rx.try_recv() {
                match r {
                    Ok(d) => {
                        self.data = d;
                        self.error = None;
                    }
                    Err(e) => {
                        crate::diag::warn(&format!("List failed to load: {e}"));
                        self.error = Some(e)
                    }
                }
                self.loading = false;
                self.loaded_at = Some(Instant::now());
                self.rx = None;
            }
        }
    }
    pub fn age(&self) -> Option<Duration> {
        self.loaded_at.map(|t| t.elapsed())
    }
}

// ───────────────────────────── App ─────────────────────────────

pub struct App {
    pub mon: Monitor,
    pub settings: Settings,
    pub pal: Palette,
    pub page: Page,
    pub snap: Arc<Snapshot>,
    pub last_seq: u64,

    // Processes page
    pub search: String,
    pub focus_search: bool,
    pub selected: Option<u32>,
    pub selected_group: Option<String>,
    pub collapsed: HashSet<u32>,
    pub expanded_groups: HashSet<String>,
    /// Bumped whenever something that affects the process rows changes.
    pub rows_gen: u64,
    pub rows_cache: Option<(
        crate::ui::processes::RowsKey,
        std::rc::Rc<Vec<crate::ui::processes::Row>>,
    )>,
    pub scroll_to_selected: bool,
    pub priority_cache: std::collections::HashMap<u32, String>,
    pub me: String,

    // Other pages
    pub perf_sel: PerfSel,
    pub perf_cores: bool,
    pub gpu_engines_view: bool,
    pub startup: AsyncList<StartupItem>,
    pub services: AsyncList<Service>,
    pub sched: AsyncList<platform::SchedTask>,
    pub startup_tab: usize,
    pub show_ms_tasks: bool,
    pub conns: AsyncList<Conn>,
    pub svc_filter: String,
    pub svc_state: usize,
    pub svc_selected: Option<String>,
    pub conn_filter: String,
    pub conn_external_only: bool,
    pub conn_established_only: bool,
    pub conn_auto: bool,
    pub log_filter: String,
    pub log_kinds: [bool; 4],

    // GPU / security / threads
    pub suspended: HashSet<u32>,
    suspended_seq: u64,
    pub sigs: std::collections::HashMap<String, platform::SigInfo>,
    pub sig_rx:
        Option<Receiver<Result<std::collections::HashMap<String, platform::SigInfo>, String>>>,
    pub hashes: std::collections::HashMap<u32, String>,
    pub threads: Option<(u32, Vec<crate::native::ThreadInfo>)>,
    pub threads_rx: Option<Receiver<(u32, Result<Vec<crate::native::ThreadInfo>, String>)>>,

    // Tools
    pub pick_at: Option<Instant>,
    pub repair_search: String,
    pub repair_cat: Option<crate::repair::Cat>,
    pub repair_confirm: Option<&'static str>,
    pub picked: Option<crate::native::WindowInfo>,
    pub windows_list: Vec<crate::native::WindowInfo>,
    pub lock_paths: Vec<std::path::PathBuf>,
    pub lockers: AsyncList<crate::native::Locker>,

    // Rules & history
    pub rule_draft: RuleDraft,
    pub history_range: i64,
    /// (range, newest point time, points) cached for the History page.
    pub history_view: Option<(i64, i64, std::rc::Rc<Vec<crate::longterm::Point>>)>,

    // Dialogs & feedback
    pub confirm: Option<Confirm>,
    pub affinity: Option<AffinityDlg>,
    pub run_dlg: Option<RunDlg>,
    pub add_startup: Option<AddStartupDlg>,
    pub install: Option<InstallDlg>,
    pub install_done: Option<std::sync::mpsc::Receiver<Result<std::path::PathBuf, String>>>,
    pub start_at_login: bool,
    pub toasts: Vec<Toast>,
    job_tx: Sender<JobResult>,
    job_rx: Receiver<JobResult>,
    pub ctx: egui::Context,

    // Window
    pub mini: bool,
    pub mini_request: bool,
    #[cfg(windows)]
    pub tray: Option<crate::tray::Tray>,
    hidden_hint_shown: bool,
    /// Smoothed UI frame cost in ms (shown in Diagnostics).
    pub frame_ms: f32,
    frames: u64,
    profile: bool,
    profile_at: Instant,
    pub uninstall_request: bool,
    restore_size: Option<egui::Vec2>,
    applied_on_top: Option<bool>,
    applied_theme: Option<(bool, usize)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, eframe::APP_KEY))
            .unwrap_or_default();
        if !settings.columns_v2 {
            for c in [Col::Gpu, Col::Net] {
                if !settings.columns.contains(&c) {
                    settings.columns.push(c);
                }
            }
            settings
                .columns
                .sort_by_key(|x| Col::ALL.iter().position(|y| y == x));
            settings.columns_v2 = true;
        }
        let ctx = cc.egui_ctx.clone();
        // One layout pass per frame: egui's default second "discard" pass
        // roughly doubles the frame cost for no visible benefit here.
        ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::MIN);
        let repaint_ctx = ctx.clone();
        let mon = Monitor::start(settings.refresh_ms, move || {
            // A tiny non-zero delay makes egui paint once per sample instead of twice.
            repaint_ctx.request_repaint_after(std::time::Duration::from_millis(1))
        });
        let (job_tx, job_rx) = channel();
        let pal = Palette::new(settings.dark, settings.accent);
        // Optional start page, e.g. TASKFORGE_PAGE=performance (handy for shortcuts).
        let start_page = std::env::var("TASKFORGE_PAGE")
            .ok()
            .and_then(|v| {
                Page::ALL
                    .into_iter()
                    .find(|p| p.label().to_lowercase().starts_with(&v.to_lowercase()))
            })
            .unwrap_or(Page::Overview);
        let me = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_default();
        App {
            snap: mon.snapshot(),
            mon,
            pal,
            page: start_page,
            selected: std::env::var("TASKFORGE_SELECT")
                .ok()
                .and_then(|v| v.parse().ok()),
            last_seq: u64::MAX,
            search: String::new(),
            focus_search: false,
            selected_group: None,
            collapsed: HashSet::new(),
            expanded_groups: HashSet::new(),
            rows_gen: 0,
            rows_cache: None,
            scroll_to_selected: false,
            priority_cache: Default::default(),
            me,
            // TASKFORGE_PERF=gpu opens the GPU view (handy for shortcuts and testing).
            perf_sel: if std::env::var("TASKFORGE_PERF").is_ok_and(|v| v == "gpu") {
                PerfSel::Gpu(0)
            } else {
                PerfSel::Cpu
            },
            perf_cores: false,
            gpu_engines_view: false,
            startup: Default::default(),
            services: Default::default(),
            sched: Default::default(),
            startup_tab: 0,
            show_ms_tasks: false,
            conns: Default::default(),
            svc_filter: String::new(),
            svc_state: 0,
            svc_selected: None,
            conn_filter: String::new(),
            conn_external_only: false,
            conn_established_only: false,
            conn_auto: true,
            log_filter: String::new(),
            log_kinds: [true; 4],
            suspended: HashSet::new(),
            suspended_seq: 0,
            sigs: Default::default(),
            sig_rx: None,
            hashes: Default::default(),
            threads: None,
            threads_rx: None,
            pick_at: None,
            repair_search: String::new(),
            repair_cat: None,
            repair_confirm: None,
            picked: None,
            windows_list: Vec::new(),
            lock_paths: Vec::new(),
            lockers: Default::default(),
            rule_draft: RuleDraft::default(),
            history_range: 3600,
            history_view: None,
            confirm: None,
            affinity: None,
            run_dlg: None,
            add_startup: None,
            install: None,
            install_done: None,
            start_at_login: platform::start_at_login_enabled(),
            toasts: Vec::new(),
            job_tx,
            job_rx,
            ctx,
            mini: false,
            // `taskforge --mini` starts as the compact always-on-top monitor.
            mini_request: std::env::args().any(|a| a == "--mini"),
            #[cfg(windows)]
            tray: None,
            hidden_hint_shown: false,
            frame_ms: 0.0,
            frames: 0,
            profile: std::env::var_os("TASKFORGE_PROFILE").is_some(),
            profile_at: Instant::now(),
            uninstall_request: false,
            restore_size: None,
            applied_on_top: None,
            applied_theme: None,
            settings,
        }
        .with_install_prompt()
        .with_tray()
    }

    #[allow(unused_mut)]
    fn with_tray(mut self) -> Self {
        #[cfg(windows)]
        if self.settings.tray_icon {
            self.tray = Some(crate::tray::Tray::start(
                self.mon.shared.clone(),
                self.mon.paused.clone(),
                self.ctx.clone(),
            ));
        }
        self
    }

    /// Tray integration: remember our window handle, run queued tray commands,
    /// and turn "close" into "hide to tray" when enabled.
    fn tray_frame(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let _ = frame;
        #[cfg(windows)]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let Some(tray) = self.tray.clone() else {
                return;
            };
            if tray.hwnd.load(Ordering::Relaxed) == 0 {
                if let Ok(h) = frame.window_handle() {
                    if let RawWindowHandle::Win32(w) = h.as_raw() {
                        tray.hwnd.store(w.hwnd.get(), Ordering::Relaxed);
                    }
                }
            }
            let cmds: Vec<crate::tray::TrayCmd> = tray
                .pending
                .lock()
                .map(|mut p| std::mem::take(&mut *p))
                .unwrap_or_default();
            for c in cmds {
                match c {
                    crate::tray::TrayCmd::Mini => self.set_mini(ctx, true),
                    crate::tray::TrayCmd::Health => {
                        self.set_mini(ctx, false);
                        self.page = Page::Health;
                    }
                }
            }
            let quitting = tray.quit.load(Ordering::Relaxed);
            if ctx.input(|i| i.viewport().close_requested())
                && self.settings.close_to_tray
                && !quitting
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                if !self.hidden_hint_shown {
                    self.hidden_hint_shown = true;
                    crate::diag::info("Window hidden to tray");
                    self.toast(ToastKind::Info, "TaskForge is still running in the tray (bottom-right). Right-click the icon to quit.");
                }
            }
        }
        let _ = (&self.hidden_hint_shown, ctx);
    }

    /// Offer to install when launched from outside the install folder (e.g. Downloads).
    fn with_install_prompt(mut self) -> Self {
        if platform::IS_WINDOWS
            && !platform::running_installed()
            && !self.settings.install_prompt_dismissed
        {
            self.install = Some(InstallDlg {
                desktop: true,
                login: false,
                busy: false,
            });
        }
        self
    }

    pub fn start_install(&mut self, desktop: bool, login: bool) {
        let (tx, rx) = channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(platform::install(desktop, login));
            ctx.request_repaint();
        });
        self.install_done = Some(rx);
    }

    fn poll_install(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.install_done else { return };
        let Ok(res) = rx.try_recv() else { return };
        self.install_done = None;
        match res {
            Ok(exe) => {
                self.start_at_login = platform::start_at_login_enabled();
                if platform::running_installed() {
                    self.install = None;
                    self.toast(ToastKind::Ok, "TaskForge is installed");
                } else {
                    // Hand over to the installed copy so shortcuts and this window match.
                    match platform::launch(&exe) {
                        Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => {
                            self.install = None;
                            self.toast(
                                ToastKind::Ok,
                                format!("Installed to {} (could not relaunch: {e})", exe.display()),
                            );
                        }
                    }
                }
            }
            Err(e) => {
                if let Some(d) = self.install.as_mut() {
                    d.busy = false;
                }
                self.toast(ToastKind::Err, format!("Install failed: {e}"));
            }
        }
    }

    // ───────────── Jobs ─────────────

    /// Run `f` on a worker thread; its result becomes a toast and optional follow-up.
    pub fn job(
        &self,
        pid: u32,
        name: &str,
        follow: Follow,
        quiet: bool,
        f: impl FnOnce() -> Result<String, String> + Send + 'static,
    ) {
        let tx = self.job_tx.clone();
        let ctx = self.ctx.clone();
        let name = name.to_string();
        std::thread::spawn(move || {
            let res = f();
            let _ = tx.send(JobResult {
                res,
                follow,
                pid,
                name,
                quiet,
            });
            ctx.request_repaint();
        });
    }

    fn poll_jobs(&mut self) {
        while let Ok(j) = self.job_rx.try_recv() {
            match (&j.follow, &j.res) {
                (Follow::AffinityLoaded(pid), Ok(s)) => {
                    if let Some(d) = self.affinity.as_mut().filter(|d| d.pid == *pid) {
                        d.mask = s.parse().ok();
                    }
                    continue;
                }
                (Follow::PriorityLoaded(pid), r) => {
                    let v = r.clone().unwrap_or_else(|_| "Unavailable".into());
                    self.priority_cache.insert(*pid, v);
                    continue;
                }
                (Follow::Hash(pid), Ok(h)) => {
                    self.hashes.insert(*pid, h.clone());
                    platform::open_url(&platform::virustotal_url(h));
                    continue;
                }
                (Follow::Suspend(pid, on), Ok(_)) => {
                    if *on {
                        self.suspended.insert(*pid);
                    } else {
                        self.suspended.remove(pid);
                    }
                }
                _ => {}
            }
            match &j.res {
                Ok(msg) => {
                    if !j.quiet {
                        self.toast(ToastKind::Ok, msg.clone());
                    }
                    self.mon.log_action(j.pid, &j.name, msg.clone());
                }
                Err(e) => {
                    crate::diag::warn(&format!(
                        "Action failed for {} (pid {}): {e}",
                        j.name, j.pid
                    ));
                    let hint = if e.to_lowercase().contains("denied") && !platform::is_admin() {
                        " — try “Restart as administrator”"
                    } else {
                        ""
                    };
                    self.toast(ToastKind::Err, format!("{}: {e}{hint}", j.name));
                }
            }
            match j.follow {
                Follow::Startup => self.load_startup(),
                Follow::Services => self.load_services(),
                Follow::Sched => self
                    .sched
                    .load(&self.ctx.clone(), platform::list_scheduled_startup),
                Follow::Priority(pid) => self.fetch_priority(pid),
                _ => {}
            }
            self.mon.refresh_now();
        }
    }

    /// Run `f` on a worker thread and hand back a receiver for its result.
    pub fn spawn<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> Receiver<T> {
        let (tx, rx) = channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
        rx
    }

    // ───────────── Suspend, restart, dump, security, threads ─────────────

    pub fn set_suspended(&mut self, pid: u32, name: &str, suspend: bool) {
        let n = name.to_string();
        self.job(pid, name, Follow::Suspend(pid, suspend), false, move || {
            if suspend {
                crate::native::suspend(pid).map(|_| format!("Suspended {n}"))
            } else {
                crate::native::resume(pid).map(|_| format!("Resumed {n}"))
            }
        });
    }

    pub fn restart(&mut self, pid: u32) {
        let Some(p) = self.snap.procs.iter().find(|p| p.pid == pid).cloned() else {
            return;
        };
        self.job(pid, &p.name, Follow::None, false, move || {
            platform::restart_process(pid, &p.exe, &p.args, &p.cwd)
        });
    }

    pub fn create_dump(&mut self, pid: u32, name: &str) {
        // Core Windows processes (including the sign-in/security process) hold
        // passwords and keys in memory; Windows blocks dumping them and
        // security software treats any attempt as an attack.
        if Self::is_critical(name) {
            self.toast(
                ToastKind::Err,
                format!("{name} is a core Windows process. TaskForge doesn't create memory dumps of these"),
            );
            return;
        }
        let stem = name.trim_end_matches(".exe");
        let file = format!(
            "{stem}-{pid}-{}.dmp",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        );
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(file)
            .add_filter("Dump file", &["dmp"])
            .save_file()
        else {
            return;
        };
        self.toast(
            ToastKind::Info,
            format!("Writing dump of {name}… (can take a while for big apps)"),
        );
        self.job(pid, name, Follow::None, false, move || {
            crate::native::write_dump(pid, &path).map(|_| {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                format!("Saved {} ({})", path.display(), widgets::fmt_bytes(size))
            })
        });
    }

    pub fn virustotal(&mut self, pid: u32, name: &str, exe: &str) {
        if let Some(h) = self.hashes.get(&pid) {
            platform::open_url(&platform::virustotal_url(h));
            return;
        }
        if exe.is_empty() {
            self.toast(
                ToastKind::Err,
                "The file path isn't available (try running as administrator)",
            );
            return;
        }
        let e = exe.to_string();
        self.job(pid, name, Follow::Hash(pid), true, move || {
            platform::sha256_file(&e)
        });
    }

    /// Verify signatures of every running executable (Process Explorer's "Verify").
    pub fn verify_all_signatures(&mut self) {
        if self.sig_rx.is_some() {
            return;
        }
        let mut paths: Vec<String> = self
            .snap
            .procs
            .iter()
            .map(|p| p.exe.to_string())
            .filter(|e| !e.is_empty())
            .collect();
        paths.sort();
        paths.dedup();
        paths.retain(|p| !self.sigs.contains_key(&p.to_lowercase()));
        self.toast(
            ToastKind::Info,
            format!("Checking signatures of {} programs…", paths.len()),
        );
        self.sig_rx = Some(self.spawn(move || platform::verify_signatures(&paths)));
        if !self.settings.columns.contains(&Col::Publisher) {
            self.settings.columns.push(Col::Publisher);
            self.settings
                .columns
                .sort_by_key(|x| Col::ALL.iter().position(|y| y == x));
        }
    }

    pub fn sig_for(&self, exe: &str) -> Option<&platform::SigInfo> {
        self.sigs.get(&exe.to_lowercase())
    }

    pub fn load_threads(&mut self, pid: u32) {
        self.threads_rx = Some(self.spawn(move || (pid, crate::native::threads(pid))));
    }

    pub fn thread_action(&mut self, pid: u32, tid: u32, a: crate::native::ThreadAction) {
        let ctx = self.ctx.clone();
        let tx = self.job_tx.clone();
        std::thread::spawn(move || {
            let res = crate::native::thread_action(tid, a).map(|_| format!("Thread {tid}: {a:?}"));
            let _ = tx.send(JobResult {
                res,
                follow: Follow::None,
                pid,
                name: format!("Thread {tid}"),
                quiet: false,
            });
            ctx.request_repaint();
        });
        self.load_threads(pid);
    }

    fn poll_misc(&mut self) {
        if let Some(rx) = &self.sig_rx {
            if let Ok(r) = rx.try_recv() {
                self.sig_rx = None;
                match r {
                    Ok(m) => {
                        let bad = m.values().filter(|s| !s.verified()).count();
                        let n = m.len();
                        self.sigs.extend(m);
                        self.rows_gen += 1;
                        self.toast(
                            ToastKind::Ok,
                            format!(
                                "Checked {n} programs: {} verified, {bad} not verified",
                                n - bad
                            ),
                        );
                    }
                    Err(e) => self.toast(ToastKind::Err, e),
                }
            }
        }
        if let Some(rx) = &self.threads_rx {
            if let Ok((pid, r)) = rx.try_recv() {
                self.threads_rx = None;
                match r {
                    Ok(mut t) => {
                        t.sort_by(|a, b| b.cpu_ms.cmp(&a.cpu_ms));
                        self.threads = Some((pid, t));
                    }
                    Err(e) => self.toast(ToastKind::Err, e),
                }
            }
        }
        if let Some(at) = self.pick_at {
            if at.elapsed() >= Duration::from_secs(3) {
                self.pick_at = None;
                self.picked = crate::native::window_at_cursor();
                if self.picked.is_none() {
                    self.toast(ToastKind::Err, "No window found under the pointer");
                }
            } else {
                self.ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        self.lockers.poll();
        // Forget suspended state for processes that are gone (once per new sample).
        if !self.suspended.is_empty() && self.snap.seq != self.suspended_seq {
            self.suspended_seq = self.snap.seq;
            let alive: HashSet<u32> = self.snap.procs.iter().map(|p| p.pid).collect();
            self.suspended.retain(|p| alive.contains(p));
        }
    }

    /// Push rule / alert settings to the background engine and show its notices.
    fn sync_automation(&mut self) {
        let cfg = crate::automation::AutoConfig {
            rules: self.settings.rules.clone(),
            alerts_enabled: self.settings.alerts_enabled,
            alert_cpu: self.settings.alert_cpu,
            alert_mem: self.settings.alert_mem,
            alert_secs: self.settings.alert_secs,
        };
        // try_lock: never stall a frame while the sampler is busy running a rule action.
        let notices = match self.mon.automation.try_lock() {
            Ok(mut a) => {
                a.configure(cfg);
                a.take_notices()
            }
            Err(_) => Vec::new(),
        };
        for n in notices {
            use crate::automation::NoticeKind as K;
            let kind = match n.kind {
                K::Ok => ToastKind::Ok,
                K::Err | K::Alert => ToastKind::Err,
                K::Info => ToastKind::Info,
            };
            self.toast(kind, n.text);
        }
    }

    pub fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.toasts.push(Toast {
            text: text.into(),
            kind,
            at: Instant::now(),
        });
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    pub fn load_startup(&mut self) {
        self.startup.load(&self.ctx, platform::list_startup);
    }
    pub fn load_services(&mut self) {
        self.services.load(&self.ctx, platform::list_services);
    }
    pub fn load_conns(&mut self) {
        self.conns.load(&self.ctx, platform::list_connections);
    }

    pub fn fetch_priority(&mut self, pid: u32) {
        self.priority_cache.insert(pid, "…".into());
        self.job(pid, "", Follow::PriorityLoaded(pid), true, move || {
            platform::get_priority(pid)
        });
    }

    // ───────────── Process actions ─────────────

    pub fn is_critical(name: &str) -> bool {
        const CRIT: [&str; 10] = [
            "system",
            "csrss.exe",
            "wininit.exe",
            "winlogon.exe",
            "smss.exe",
            "services.exe",
            "lsass.exe",
            "svchost.exe",
            "dwm.exe",
            "registry",
        ];
        let n = name.to_ascii_lowercase();
        CRIT.contains(&n.as_str())
    }

    pub fn request_kill(&mut self, pid: u32, name: &str, tree: bool) {
        let crit = Self::is_critical(name);
        if self.settings.confirm_kill || crit {
            let what = if tree {
                "this process and all of its children"
            } else {
                "this process"
            };
            self.confirm = Some(Confirm {
                title: format!("End {name}?"),
                body: format!(
                    "Ending {what} (PID {pid}) closes it immediately. Unsaved work will be lost."
                ),
                warn: crit.then(|| {
                    "This is a core Windows process. Ending it may make Windows unstable or restart your PC."
                        .to_string()
                }),
                action: ConfirmAction::Kill { pid, name: name.to_string(), tree },
            });
        } else {
            self.do_kill(pid, name, tree);
        }
    }

    pub fn request_kill_all(&mut self, name: &str) {
        let pids: Vec<u32> = self
            .snap
            .procs
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case(name))
            .map(|p| p.pid)
            .collect();
        if pids.is_empty() {
            return;
        }
        let crit = Self::is_critical(name);
        if self.settings.confirm_kill || crit {
            self.confirm = Some(Confirm {
                title: format!("End all {name}?"),
                body: format!(
                    "{} instance(s) of {name} will be closed immediately.",
                    pids.len()
                ),
                warn: crit.then(|| "This is a core Windows process.".to_string()),
                action: ConfirmAction::KillAll {
                    name: name.to_string(),
                    pids,
                },
            });
        } else {
            self.do_kill_all(name, pids);
        }
    }

    fn do_kill(&mut self, pid: u32, name: &str, tree: bool) {
        let n = name.to_string();
        self.job(pid, name, Follow::None, false, move || {
            platform::kill(pid, tree).map(|_| {
                if tree {
                    format!("Ended {n} and its children")
                } else {
                    format!("Ended {n}")
                }
            })
        });
        if self.selected == Some(pid) {
            self.selected = None;
        }
    }

    fn do_kill_all(&mut self, name: &str, pids: Vec<u32>) {
        let n = name.to_string();
        self.job(0, name, Follow::None, false, move || {
            let total = pids.len();
            let errors: Vec<String> = pids
                .into_iter()
                .filter_map(|p| platform::kill(p, false).err())
                .collect();
            if errors.len() == total {
                Err(errors.into_iter().next().unwrap_or_default())
            } else {
                Ok(format!(
                    "Ended {} of {total} {n} process(es)",
                    total - errors.len()
                ))
            }
        });
    }

    pub fn set_priority(&mut self, pid: u32, name: &str, p: platform::Priority) {
        self.job(pid, name, Follow::Priority(pid), false, move || {
            platform::set_priority(pid, p)
        });
    }

    pub fn open_affinity(&mut self, pid: u32, name: &str) {
        self.affinity = Some(AffinityDlg {
            pid,
            name: name.to_string(),
            mask: None,
        });
        self.job(pid, name, Follow::AffinityLoaded(pid), true, move || {
            platform::get_affinity(pid).map(|m| m.to_string())
        });
    }

    pub fn toggle_pin(&mut self, name: &str) {
        let key = name.to_lowercase();
        if let Some(i) = self.settings.pinned.iter().position(|p| *p == key) {
            self.settings.pinned.remove(i);
            self.rows_gen += 1;
        } else {
            self.settings.pinned.push(key);
            self.rows_gen += 1;
        }
    }

    pub fn is_pinned(&self, name: &str) -> bool {
        let key = name.to_lowercase();
        self.settings.pinned.contains(&key)
    }

    pub fn reveal_process(&mut self, pid: u32) {
        self.page = Page::Processes;
        self.selected = Some(pid);
        self.selected_group = None;
        self.search.clear();
        self.scroll_to_selected = true;
        if let Some(p) = self.snap.procs.iter().find(|p| p.pid == pid) {
            self.expanded_groups.insert(p.name.to_lowercase());
            self.rows_gen += 1;
        }
    }

    pub fn export_csv(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!(
                "processes-{}.csv",
                chrono::Local::now().format("%Y%m%d-%H%M%S")
            ))
            .add_filter("CSV", &["csv"])
            .save_file()
        else {
            return;
        };
        let esc = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let mut out = String::from("Name,PID,ParentPID,User,CPU %,Memory (bytes),Disk read B/s,Disk write B/s,Started,Path,Command line\n");
        for p in &self.snap.procs {
            out.push_str(&format!(
                "{},{},{},{},{:.2},{},{},{},{},{},{}\n",
                esc(&p.name),
                p.pid,
                p.parent.map(|x| x.to_string()).unwrap_or_default(),
                esc(&p.user),
                p.cpu,
                p.mem,
                p.disk_read,
                p.disk_write,
                esc(&widgets::fmt_epoch(p.start_time)),
                esc(&p.exe),
                esc(&p.cmd)
            ));
        }
        match std::fs::write(&path, out) {
            Ok(_) => self.toast(
                ToastKind::Ok,
                format!("Exported {} processes", self.snap.procs.len()),
            ),
            Err(e) => self.toast(ToastKind::Err, format!("Export failed: {e}")),
        }
    }

    // ───────────── Window state ─────────────

    fn sync_window(&mut self, ctx: &egui::Context) {
        let on_top = self.settings.always_on_top || self.mini;
        if self.applied_on_top != Some(on_top) {
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(if on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            }));
            self.applied_on_top = Some(on_top);
        }
        let th = (self.settings.dark, self.settings.accent);
        if self.applied_theme != Some(th) {
            self.pal = Palette::new(th.0, th.1);
            theme::apply(ctx, &self.pal);
            self.applied_theme = Some(th);
        }
    }

    pub fn set_mini(&mut self, ctx: &egui::Context, mini: bool) {
        if mini == self.mini {
            return;
        }
        if mini {
            self.restore_size = ctx.input(|i| i.viewport().inner_rect.map(|r| r.size()));
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(vec2(260.0, 150.0)));
            let h = if self.snap.gpus.is_empty() {
                214.0
            } else {
                256.0
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(300.0, h)));
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(vec2(900.0, 560.0)));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                self.restore_size.unwrap_or(vec2(1280.0, 800.0)),
            ));
        }
        self.mini = mini;
    }

    // ───────────── Shortcuts ─────────────

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.wants_keyboard_input();
        let mut actions = Vec::new();
        ctx.input(|i| {
            let cmd = i.modifiers.command;
            if cmd && i.key_pressed(Key::F) {
                actions.push("search");
            }
            if cmd && i.key_pressed(Key::E) {
                actions.push("export");
            }
            if cmd && i.key_pressed(Key::R) {
                actions.push("run");
            }
            if cmd && i.key_pressed(Key::P) {
                actions.push("pause");
            }
            if cmd && i.key_pressed(Key::M) {
                actions.push("mini");
            }
            if i.key_pressed(Key::F5) {
                actions.push("refresh");
            }
            if !typing && i.key_pressed(Key::Delete) {
                actions.push(if i.modifiers.shift {
                    "kill_tree"
                } else {
                    "kill"
                });
            }
            if i.key_pressed(Key::Escape) {
                actions.push("escape");
            }
            let nums = [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
            ];
            for (k, page) in nums.iter().zip(Page::ALL) {
                if cmd && i.key_pressed(*k) {
                    self.page = page;
                }
            }
        });
        for a in actions {
            match a {
                "search" => {
                    if self.mini {
                        self.set_mini(ctx, false);
                    }
                    self.page = Page::Processes;
                    self.focus_search = true;
                }
                "export" => self.export_csv(),
                "run" => {
                    self.run_dlg = Some(RunDlg {
                        focus: true,
                        ..Default::default()
                    })
                }
                "pause" => self.toggle_pause(),
                "mini" => self.set_mini(ctx, !self.mini),
                "refresh" => {
                    self.mon.refresh_now();
                    match self.page {
                        Page::Startup => self.load_startup(),
                        Page::Services => self.load_services(),
                        Page::Network => self.load_conns(),
                        _ => {}
                    }
                }
                "kill" | "kill_tree" if self.page == Page::Processes && self.confirm.is_none() => {
                    if let Some(pid) = self.selected {
                        if let Some(p) = self.snap.procs.iter().find(|p| p.pid == pid) {
                            let n = p.name.clone();
                            self.request_kill(pid, &n, a == "kill_tree");
                        }
                    } else if let Some(g) = self.selected_group.clone() {
                        self.request_kill_all(&g);
                    }
                }
                "escape" => {
                    if self.confirm.is_some()
                        || self.affinity.is_some()
                        || self.run_dlg.is_some()
                        || self.add_startup.is_some()
                    {
                        self.confirm = None;
                        self.affinity = None;
                        self.run_dlg = None;
                        self.add_startup = None;
                    } else if !self.search.is_empty() {
                        self.search.clear();
                    } else {
                        self.selected = None;
                        self.selected_group = None;
                    }
                }
                _ => {}
            }
        }
    }

    pub fn toggle_pause(&mut self) {
        let p = !self.mon.paused.load(Ordering::Relaxed);
        self.mon.paused.store(p, Ordering::Relaxed);
    }

    pub fn paused(&self) -> bool {
        self.mon.paused.load(Ordering::Relaxed)
    }

    /// Number of samples that fit in the configured graph window.
    pub fn graph_window(&self) -> usize {
        ((self.settings.graph_secs * 1000) / self.settings.refresh_ms.max(1))
            .clamp(10, crate::monitor::HISTORY_CAP as u64) as usize
    }

    // ───────────── Shell UI ─────────────

    fn sidebar(&mut self, ctx: &egui::Context) {
        let pal = self.pal;
        egui::SidePanel::left("nav")
            .exact_width(206.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(pal.sidebar)
                    .inner_margin(Margin::symmetric(12, 14)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(30.0, 30.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 9.0, pal.accent);
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        "⚡",
                        egui::FontId::proportional(17.0),
                        Color32::WHITE,
                    );
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(RichText::new("TaskForge").size(16.0).strong());
                        ui.label(
                            RichText::new("Task Manager, upgraded")
                                .size(10.5)
                                .color(pal.text_faint),
                        );
                    });
                });
                ui.add_space(18.0);

                for page in Page::ALL {
                    if page == Page::Settings {
                        continue;
                    }
                    self.nav_button(ui, page);
                }

                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    self.nav_button(ui, Page::Settings);
                    ui.add_space(6.0);
                    self.sidebar_meters(ui);
                });
            });
    }

    fn nav_button(&mut self, ui: &mut egui::Ui, page: Page) {
        let pal = self.pal;
        let sel = self.page == page;
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), 36.0), egui::Sense::click());
        let bg = if sel {
            widgets::with_alpha(pal.accent, if pal.dark { 0.18 } else { 0.14 })
        } else if resp.hovered() {
            widgets::with_alpha(pal.text, 0.05)
        } else {
            Color32::TRANSPARENT
        };
        let p = ui.painter();
        p.rect_filled(rect, 8.0, bg);
        if sel {
            let bar = egui::Rect::from_min_size(rect.left_top() + vec2(0.0, 9.0), vec2(3.0, 18.0));
            p.rect_filled(bar, 2.0, pal.accent);
        }
        let col = if sel { pal.text } else { pal.text_dim };
        p.text(
            rect.left_center() + vec2(16.0, 0.0),
            egui::Align2::LEFT_CENTER,
            page.icon(),
            egui::FontId::proportional(15.0),
            if sel { pal.accent } else { col },
        );
        p.text(
            rect.left_center() + vec2(42.0, 0.0),
            egui::Align2::LEFT_CENTER,
            page.label(),
            egui::FontId::proportional(13.5),
            col,
        );

        // Live badges.
        let badge = match page {
            Page::Processes => Some(self.snap.procs.len().to_string()),
            Page::Network if !self.conns.data.is_empty() => Some(self.conns.data.len().to_string()),
            _ => None,
        };
        if let Some(b) = badge {
            p.text(
                rect.right_center() - vec2(10.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                b,
                egui::FontId::proportional(11.0),
                pal.text_faint,
            );
        }
        if resp.clicked() {
            self.page = page;
        }
        ui.add_space(2.0);
    }

    fn sidebar_meters(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let s = self.snap.clone();
        let rows = [
            (
                "CPU",
                s.cpu_total / 100.0,
                format!("{:.0}%", s.cpu_total),
                pal.cpu,
            ),
            (
                "Memory",
                s.mem_pct() / 100.0,
                format!("{:.0}%", s.mem_pct()),
                pal.mem,
            ),
        ];
        egui::Frame::new()
            .fill(pal.card)
            .corner_radius(10.0)
            .stroke(Stroke::new(1.0f32, pal.border))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (label, frac, text, col) in rows.into_iter().rev() {
                    widgets::bar(ui, &pal, frac, col, 5.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(label).size(11.5).color(pal.text_dim));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new(text).size(11.5).strong());
                        });
                    });
                }
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("⬇ {}", fmt_rate(s.net_rx_rate as f32)))
                            .size(10.5)
                            .color(pal.net_rx),
                    );
                    ui.label(
                        RichText::new(format!("⬆ {}", fmt_rate(s.net_tx_rate as f32)))
                            .size(10.5)
                            .color(pal.net_tx),
                    );
                });
            });
    }

    fn header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new(self.page.label()).size(22.0).strong());
                ui.label(RichText::new(self.page.subtitle()).size(12.0).color(pal.text_faint));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(if pal.dark { "☀" } else { "🌙" }).on_hover_text("Toggle light / dark").clicked() {
                    self.settings.dark = !self.settings.dark;
                }
                if ui.button("🗗").on_hover_text("Mini monitor — compact always-on-top view (Ctrl+M)").clicked() {
                    self.set_mini(ctx, true);
                }
                let pin_txt = RichText::new("📌").color(if self.settings.always_on_top { pal.accent } else { pal.text_dim });
                if ui.button(pin_txt).on_hover_text("Always on top").clicked() {
                    self.settings.always_on_top = !self.settings.always_on_top;
                }
                let paused = self.paused();
                let (txt, tip) = if paused { ("▶ Resume", "Resume live updates (Ctrl+P)") } else { ("⏸ Pause", "Freeze the view (Ctrl+P)") };
                let b = egui::Button::new(RichText::new(txt).color(if paused { pal.warn } else { pal.text }));
                if ui.add(b).on_hover_text(tip).clicked() {
                    self.toggle_pause();
                }
                if ui.button("▶ Run new task").on_hover_text("Ctrl+R").clicked() {
                    self.run_dlg = Some(RunDlg { focus: true, ..Default::default() });
                }
                if self.page == Page::Overview
                    && theme::primary_button(ui, &pal, "❤ Why is my PC slow?").clicked()
                {
                    self.page = Page::Health;
                }
                if !platform::is_admin() && platform::IS_WINDOWS {
                    let r = ui.add(egui::Button::new(RichText::new("🛡 Restart as admin").color(pal.warn)));
                    if r.on_hover_text("Some processes, services and startup items can only be managed with administrator rights").clicked() {
                        match platform::relaunch_as_admin() {
                            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                            Err(e) => self.toast(ToastKind::Err, format!("Could not elevate: {e}")),
                        }
                    }
                } else if platform::is_admin() {
                    widgets::pill(ui, "🛡 Administrator", pal.good);
                }
            });
        });
        ui.add_space(10.0);
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        let pal = self.pal;
        let s = self.snap.clone();
        egui::TopBottomPanel::bottom("status")
            .exact_height(26.0)
            .frame(
                egui::Frame::new()
                    .fill(pal.sidebar)
                    .inner_margin(Margin::symmetric(14, 4)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    let t = |ui: &mut egui::Ui, s: String| {
                        ui.label(RichText::new(s).size(11.5).color(pal.text_dim));
                    };
                    let dot = if self.paused() { pal.warn } else { pal.good };
                    let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 4.0, dot);
                    t(
                        ui,
                        if self.paused() {
                            "Paused".into()
                        } else {
                            format!("Live · {} ms", self.settings.refresh_ms)
                        },
                    );
                    ui.separator();
                    t(ui, format!("{} processes", s.procs.len()));
                    ui.separator();
                    t(ui, format!("CPU {:.0}%", s.cpu_total));
                    ui.separator();
                    t(
                        ui,
                        format!(
                            "Memory {} / {}",
                            fmt_bytes(s.mem_used),
                            fmt_bytes(s.mem_total)
                        ),
                    );
                    ui.separator();
                    t(
                        ui,
                        format!(
                            "Disk ⬇{} ⬆{}",
                            fmt_rate(s.disk_read_rate as f32),
                            fmt_rate(s.disk_write_rate as f32)
                        ),
                    );
                    ui.separator();
                    t(ui, format!("Up {}", fmt_dur(s.uptime)));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        t(ui, "Ctrl+F search · Del end task · F5 refresh".into());
                    });
                });
            });
    }

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        let pal = self.pal;
        self.toasts.retain(|t| {
            t.at.elapsed() < Duration::from_secs(if t.kind == ToastKind::Err { 8 } else { 4 })
        });
        if self.toasts.is_empty() {
            return;
        }
        // Wake up exactly when the next toast expires (not 4× a second).
        if let Some(next) = self
            .toasts
            .iter()
            .map(|t| {
                Duration::from_secs(if t.kind == ToastKind::Err { 8 } else { 4 })
                    .saturating_sub(t.at.elapsed())
            })
            .min()
        {
            ctx.request_repaint_after(next + Duration::from_millis(20));
        }
        let mut dismiss = None;
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-16.0, -40.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(380.0);
                for (i, t) in self.toasts.iter().enumerate() {
                    let col = match t.kind {
                        ToastKind::Ok => pal.good,
                        ToastKind::Err => pal.bad,
                        ToastKind::Info => pal.accent,
                    };
                    let r = egui::Frame::new()
                        .fill(pal.card)
                        .stroke(Stroke::new(1.0f32, widgets::with_alpha(col, 0.6)))
                        .corner_radius(10.0)
                        .shadow(egui::epaint::Shadow {
                            offset: [0, 4],
                            blur: 16,
                            spread: 0,
                            color: Color32::from_black_alpha(60),
                        })
                        .inner_margin(Margin::symmetric(12, 9))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let icon = match t.kind {
                                    ToastKind::Ok => "✔",
                                    ToastKind::Err => "✖",
                                    ToastKind::Info => "ℹ",
                                };
                                ui.label(RichText::new(icon).color(col).strong());
                                ui.add(egui::Label::new(&t.text).wrap());
                            });
                        })
                        .response
                        .interact(egui::Sense::click());
                    if r.clicked() {
                        dismiss = Some(i);
                    }
                    ui.add_space(6.0);
                }
            });
        if let Some(i) = dismiss {
            self.toasts.remove(i);
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let pal = self.pal;

        // Confirmation
        if let Some(c) = self.confirm.as_ref() {
            let mut close = false;
            let mut go = false;
            let m = egui::Modal::new(egui::Id::new("confirm")).show(ctx, |ui| {
                ui.set_width(400.0);
                ui.label(RichText::new(&c.title).size(17.0).strong());
                ui.add_space(6.0);
                ui.add(egui::Label::new(&c.body).wrap());
                if let Some(w) = &c.warn {
                    ui.add_space(6.0);
                    egui::Frame::new()
                        .fill(widgets::with_alpha(pal.bad, 0.12))
                        .corner_radius(8.0)
                        .inner_margin(Margin::same(10))
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(format!("⚠ {w}")).color(pal.bad))
                                    .wrap(),
                            );
                        });
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.settings.confirm_kill, "Always ask");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if theme::danger_button(ui, &pal, "Confirm", true).clicked()
                            || ui.input(|i| i.key_pressed(Key::Enter))
                        {
                            go = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            });
            if m.should_close() {
                close = true;
            }
            if go {
                let action = c.action.clone();
                self.confirm = None;
                match action {
                    ConfirmAction::Kill { pid, name, tree } => self.do_kill(pid, &name, tree),
                    ConfirmAction::KillAll { name, pids } => self.do_kill_all(&name, pids),
                    ConfirmAction::DeleteStartup(item) => {
                        let n = item.name.clone();
                        self.job(0, &n, Follow::Startup, false, move || {
                            platform::delete_startup(&item)
                        });
                    }
                    ConfirmAction::Uninstall => self.uninstall_request = true,
                    ConfirmAction::KillThread { pid, tid } => {
                        self.thread_action(pid, tid, crate::native::ThreadAction::Kill)
                    }
                    ConfirmAction::StopService(name) => {
                        let n = name.clone();
                        self.job(0, &n, Follow::Services, false, move || {
                            platform::service_action(&name, platform::ServiceAction::Stop)
                        });
                    }
                }
            } else if close {
                self.confirm = None;
            }
        }

        // Affinity
        if let Some(d) = self.affinity.as_mut() {
            let n = self.snap.logical_cores.clamp(1, 64);
            let mut close = false;
            let mut apply = None;
            let m = egui::Modal::new(egui::Id::new("affinity")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.label(
                    RichText::new(format!("Processor affinity — {}", d.name))
                        .size(17.0)
                        .strong(),
                );
                widgets::faint(
                    ui,
                    &pal,
                    "Choose which logical processors this process may run on.",
                );
                ui.add_space(8.0);
                match d.mask.as_mut() {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Reading current affinity…");
                        });
                    }
                    Some(mask) => {
                        ui.horizontal(|ui| {
                            if ui.button("All").clicked() {
                                *mask = if n >= 64 { u64::MAX } else { (1u64 << n) - 1 };
                            }
                            if ui.button("None").clicked() {
                                *mask = 0;
                            }
                            if ui.button("Even only").clicked() {
                                *mask = (0..n).step_by(2).fold(0, |m, i| m | 1 << i);
                            }
                        });
                        egui::Grid::new("aff")
                            .num_columns(4)
                            .spacing(vec2(18.0, 6.0))
                            .show(ui, |ui| {
                                for i in 0..n {
                                    let mut on = *mask & (1 << i) != 0;
                                    if ui.checkbox(&mut on, format!("CPU {i}")).changed() {
                                        if on {
                                            *mask |= 1 << i;
                                        } else {
                                            *mask &= !(1 << i);
                                        }
                                    }
                                    if i % 4 == 3 {
                                        ui.end_row();
                                    }
                                }
                            });
                    }
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled(d.mask.is_some_and(|m| m != 0), egui::Button::new("Apply"))
                            .clicked()
                        {
                            apply = d.mask;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            });
            if m.should_close() {
                close = true;
            }
            if let Some(mask) = apply {
                let (pid, name) = (d.pid, d.name.clone());
                self.affinity = None;
                self.job(pid, &name, Follow::None, false, move || {
                    platform::set_affinity(pid, mask)
                });
            } else if close {
                self.affinity = None;
            }
        }

        // Run new task
        if let Some(d) = self.run_dlg.as_mut() {
            let mut close = false;
            let mut go = false;
            let m = egui::Modal::new(egui::Id::new("run")).show(ctx, |ui| {
                ui.set_width(440.0);
                ui.label(RichText::new("Run new task").size(17.0).strong());
                widgets::faint(
                    ui,
                    &pal,
                    "Type the name of a program, folder, document or website.",
                );
                ui.add_space(8.0);
                let r = ui.add(
                    egui::TextEdit::singleline(&mut d.cmd)
                        .hint_text("e.g. notepad, cmd, https://…")
                        .desired_width(f32::INFINITY),
                );
                if d.focus {
                    r.request_focus();
                    d.focus = false;
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    go = true;
                }
                ui.horizontal(|ui| {
                    if ui.button("📂 Browse…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_file() {
                            d.cmd = format!("\"{}\"", p.display());
                        }
                    }
                    ui.checkbox(
                        &mut d.admin,
                        "Create this task with administrative privileges",
                    );
                });
                ui.add_space(8.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::primary_button(ui, &pal, "Run").clicked() {
                        go = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if m.should_close() {
                close = true;
            }
            if go {
                let (cmd, admin) = (d.cmd.clone(), d.admin);
                self.run_dlg = None;
                self.job(0, "Run", Follow::None, false, move || {
                    platform::run_task(&cmd, admin)
                });
            } else if close {
                self.run_dlg = None;
            }
        }

        // Install / welcome
        if let Some(d) = self.install.as_mut() {
            let mut go = None;
            let mut skip = false;
            egui::Modal::new(egui::Id::new("install")).show(ctx, |ui| {
                ui.set_width(440.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 11.0, pal.accent);
                    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "⚡", egui::FontId::proportional(22.0), Color32::WHITE);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Welcome to TaskForge").size(18.0).strong());
                        widgets::faint(ui, &pal, "Install it so it's always one click away.");
                    });
                });
                ui.add_space(10.0);
                widgets::faint(ui, &pal, "Adds TaskForge to your Start menu and to Settings › Apps (so you can uninstall it like any app). No administrator rights needed.");
                ui.add_space(8.0);
                ui.add_enabled_ui(!d.busy, |ui| {
                    ui.checkbox(&mut d.desktop, "Create a desktop shortcut");
                    ui.checkbox(&mut d.login, "Start when I sign in (as the compact mini monitor)");
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if d.busy {
                        ui.spinner();
                        ui.label("Installing…");
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(!d.busy, egui::Button::new(RichText::new("Install").color(Color32::WHITE).strong()).fill(pal.accent)).clicked() {
                            go = Some((d.desktop, d.login));
                        }
                        if ui.add_enabled(!d.busy, egui::Button::new("Run without installing")).clicked() {
                            skip = true;
                        }
                    });
                });
            });
            if let Some((desk, login)) = go {
                d.busy = true;
                self.start_install(desk, login);
            } else if skip {
                self.install = None;
                self.settings.install_prompt_dismissed = true;
            }
        }

        // Add startup item
        if let Some(d) = self.add_startup.as_mut() {
            let mut close = false;
            let mut go = false;
            let m = egui::Modal::new(egui::Id::new("addstartup")).show(ctx, |ui| {
                ui.set_width(440.0);
                ui.label(RichText::new("Add startup app").size(17.0).strong());
                widgets::faint(
                    ui,
                    &pal,
                    "Launches automatically when you sign in (current user).",
                );
                ui.add_space(8.0);
                egui::Grid::new("add_su")
                    .num_columns(2)
                    .spacing(vec2(10.0, 8.0))
                    .show(ui, |ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(320.0));
                        ui.end_row();
                        ui.label("Command");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut d.cmd).desired_width(240.0));
                            if ui.button("Browse…").clicked() {
                                if let Some(p) = rfd::FileDialog::new()
                                    .add_filter("Programs", &["exe", "bat", "cmd", "lnk"])
                                    .pick_file()
                                {
                                    if d.name.is_empty() {
                                        d.name = p
                                            .file_stem()
                                            .map(|s| s.to_string_lossy().into_owned())
                                            .unwrap_or_default();
                                    }
                                    d.cmd = format!("\"{}\"", p.display());
                                }
                            }
                        });
                        ui.end_row();
                    });
                ui.add_space(8.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::primary_button(ui, &pal, "Add").clicked() {
                        go = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if m.should_close() {
                close = true;
            }
            if go {
                let (n, c) = (d.name.clone(), d.cmd.clone());
                self.add_startup = None;
                self.job(0, &n.clone(), Follow::Startup, false, move || {
                    platform::add_startup(&n, &c)
                });
            } else if close {
                self.add_startup = None;
            }
        }
    }

    fn mini_ui(&mut self, ctx: &egui::Context) {
        let pal = self.pal;
        let shared = self.mon.shared.clone();
        let s = self.snap.clone();
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(pal.bg)
                    .inner_margin(Margin::same(10)),
            )
            .show(ctx, |ui| {
                let g = shared.lock().unwrap();
                let h = &g.history;
                let main_gpu = (0..s.gpus.len()).max_by_key(|&i| s.gpus[i].ded_total);
                let mut rows: Vec<(
                    &str,
                    String,
                    &std::collections::VecDeque<f32>,
                    Color32,
                    Option<f32>,
                )> = vec![
                    (
                        "CPU",
                        format!("{:.0}%", s.cpu_total),
                        &h.cpu,
                        pal.cpu,
                        Some(100.0),
                    ),
                    (
                        "MEM",
                        format!("{:.0}%", s.mem_pct()),
                        &h.mem,
                        pal.mem,
                        Some(100.0),
                    ),
                    (
                        "NET",
                        fmt_rate((s.net_rx_rate + s.net_tx_rate) as f32),
                        &h.net_rx,
                        pal.net_rx,
                        None,
                    ),
                    (
                        "DISK",
                        fmt_rate((s.disk_read_rate + s.disk_write_rate) as f32),
                        &h.disk_r,
                        pal.disk,
                        None,
                    ),
                ];
                if let (Some(gi), Some(gh)) = (main_gpu, main_gpu.and_then(|i| h.gpus.get(i))) {
                    let gp = &s.gpus[gi];
                    let val = match gp.temp_c {
                        Some(t) => format!("{:.0}% {t:.0}°", gp.util),
                        None => format!("{:.0}%", gp.util),
                    };
                    rows.insert(2, ("GPU", val, &gh.util, pal.gpu, Some(100.0)));
                }
                for (label, val, data, col, max) in rows {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            vec2(34.0, 34.0),
                            egui::Label::new(RichText::new(label).size(11.0).strong().color(col)),
                        );
                        ui.add_sized(
                            vec2(72.0, 34.0),
                            egui::Label::new(RichText::new(val).size(12.5).strong()),
                        );
                        widgets::sparkline(
                            ui,
                            &pal,
                            data,
                            col,
                            max,
                            vec2(ui.available_width(), 34.0),
                        );
                    });
                }
                drop(g);
                ui.horizontal(|ui| {
                    widgets::faint(
                        ui,
                        &pal,
                        format!("{} procs · up {}", s.procs.len(), fmt_dur(s.uptime)),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("⛶ Expand").clicked() {
                            self.set_mini(ctx, false);
                        }
                    });
                });
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.tray_frame(ctx, frame);
        if let Some(cpu) = frame.info().cpu_usage {
            self.frame_ms = if self.frame_ms == 0.0 {
                cpu * 1000.0
            } else {
                self.frame_ms * 0.95 + cpu * 50.0
            };
        }
        self.frames += 1;
        if self.profile && self.profile_at.elapsed() >= Duration::from_secs(30) {
            crate::diag::info(&format!(
                "PROFILE page={:?} frames={} ({:.1}/s) frame_ms={:.2} sample_ms={:.2} procs={}",
                self.page,
                self.frames,
                self.frames as f32 / self.profile_at.elapsed().as_secs_f32(),
                self.frame_ms,
                self.snap.sample_cost_ms,
                self.snap.procs.len()
            ));
            self.frames = 0;
            self.profile_at = Instant::now();
        }
        // Push setting changes to the sampler.
        self.mon.set_interval(self.settings.refresh_ms);
        self.sync_window(ctx);

        if !self.paused() {
            self.snap = self.mon.snapshot();
        }
        if self.snap.seq != self.last_seq {
            self.last_seq = self.snap.seq;
            self.sync_automation();
        }
        self.poll_jobs();
        self.poll_misc();
        self.poll_install(ctx);
        self.startup.poll();
        self.services.poll();
        self.sched.poll();
        self.conns.poll();
        self.shortcuts(ctx);

        if self.mini_request && !self.mini && self.page != Page::Settings {
            self.mini_request = false;
            self.set_mini(ctx, true);
        }
        if self.mini {
            self.mini_ui(ctx);
            self.toasts_ui(ctx);
            return;
        }

        self.sidebar(ctx);
        self.status_bar(ctx);
        let pal = self.pal;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin {
                left: 22,
                right: 22,
                top: 16,
                bottom: 10,
            }))
            .show(ctx, |ui| {
                self.header(ui, ctx);
                match self.page {
                    Page::Overview => self.page_overview(ui),
                    Page::Processes => self.page_processes(ui),
                    Page::Performance => self.page_performance(ui),
                    Page::Network => self.page_network(ui),
                    Page::Startup => self.page_startup(ui),
                    Page::Services => self.page_services(ui),
                    Page::Activity => self.page_activity(ui),
                    Page::History => self.page_history(ui),
                    Page::Health => self.page_health(ui),
                    Page::Repair => self.page_repair(ui),
                    Page::Rules => self.page_rules(ui),
                    Page::Tools => self.page_tools(ui),
                    Page::Settings => self.page_settings(ui, ctx),
                }
            });
        self.dialogs(ctx);
        self.toasts_ui(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.mon.save_history();
        #[cfg(windows)]
        crate::native::shutdown_net();
        // Don't leave anything frozen behind.
        for pid in self.suspended.drain() {
            let _ = crate::native::resume(pid);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }
}
