//! GPU monitoring on Windows.
//!
//! * **Adapters, VRAM totals, driver version, memory budget**: DXGI.
//! * **Engine & per-process utilisation, per-process / per-adapter memory**:
//!   the "GPU Engine", "GPU Process Memory" and "GPU Adapter Memory" performance
//!   counters (PDH). These are the counters Task Manager is built on; overall
//!   load is the busiest engine.
//! * **Temperature, fan, power, clocks, voltage, bandwidth**: the WDDM 2.x
//!   kernel adapter perf data (`D3DKMTQueryAdapterInfo`). Drivers fill in what
//!   they support; anything unreported shows as unavailable.

use std::collections::HashMap;
use std::time::Instant;

use windows::core::{Interface, PCWSTR};
use windows::Wdk::Graphics::Direct3D::{
    D3DKMTCloseAdapter, D3DKMTOpenAdapterFromLuid, D3DKMTQueryAdapterInfo, D3DKMT_ADAPTER_PERFDATA,
    D3DKMT_ADAPTER_PERFDATACAPS, D3DKMT_CLOSEADAPTER, D3DKMT_NODE_PERFDATA,
    D3DKMT_OPENADAPTERFROMLUID, D3DKMT_QUERYADAPTERINFO, KMTQAITYPE_ADAPTERPERFDATA,
    KMTQAITYPE_ADAPTERPERFDATA_CAPS, KMTQAITYPE_NODEPERFDATA, KMTQUERYADAPTERINFOTYPE,
};
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIAdapter3, IDXGIDevice, IDXGIFactory1,
    DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY,
    PDH_MORE_DATA,
};

use super::{
    driver_version, engine_label, parse_gpu_instance, vendor_name, GpuAdapter, GpuEngine, GpuProc,
    GpuSample,
};

/// Re-create the PDH query periodically so engines of newly started
/// processes are picked up (wildcard expansion is not guaranteed to refresh).
const REBUILD_SECS: u64 = 20;

struct Query {
    q: PDH_HQUERY,
    engine: PDH_HCOUNTER,
    proc_ded: PDH_HCOUNTER,
    proc_shared: PDH_HCOUNTER,
    ad_ded: PDH_HCOUNTER,
    ad_shared: PDH_HCOUNTER,
    built: Instant,
    samples: u32,
}

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: the query handle came from PdhOpenQueryW and is closed once.
        unsafe {
            PdhCloseQuery(self.q);
        }
    }
}

impl Query {
    fn new() -> Option<Query> {
        let mut q = PDH_HQUERY::default();
        // SAFETY: out-param is a live local.
        if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut q) } != 0 {
            return None;
        }
        let add = |path: &str| -> PDH_HCOUNTER {
            let w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            let mut c = PDH_HCOUNTER::default();
            // SAFETY: `w` is NUL-terminated and alive for the call; `c` is a live local.
            // English names keep this working on non-English Windows.
            unsafe { PdhAddEnglishCounterW(q, PCWSTR(w.as_ptr()), 0, &mut c) };
            c
        };
        let query = Query {
            engine: add(r"\GPU Engine(*)\Utilization Percentage"),
            proc_ded: add(r"\GPU Process Memory(*)\Dedicated Usage"),
            proc_shared: add(r"\GPU Process Memory(*)\Shared Usage"),
            ad_ded: add(r"\GPU Adapter Memory(*)\Dedicated Usage"),
            ad_shared: add(r"\GPU Adapter Memory(*)\Shared Usage"),
            q,
            built: Instant::now(),
            samples: 0,
        };
        // SAFETY: valid query. The first collection only primes rate counters.
        unsafe { PdhCollectQueryData(q) };
        Some(query)
    }

    /// All (instance name, value) pairs of a wildcard counter.
    fn array(&self, c: PDH_HCOUNTER, fmt: PDH_FMT) -> Vec<(String, f64)> {
        if c.is_invalid() {
            return Vec::new();
        }
        let (mut size, mut count) = (0u32, 0u32);
        // SAFETY: size query with no buffer, as documented.
        let rc = unsafe { PdhGetFormattedCounterArrayW(c, fmt, &mut size, &mut count, None) };
        if rc != PDH_MORE_DATA || size == 0 {
            return Vec::new();
        }
        // u64 backing keeps the item array 8-byte aligned.
        let mut buf = vec![0u64; (size as usize).div_ceil(8) + 1];
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        // SAFETY: buffer is at least `size` bytes and suitably aligned.
        let rc =
            unsafe { PdhGetFormattedCounterArrayW(c, fmt, &mut size, &mut count, Some(items)) };
        if rc != 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            // SAFETY: PDH wrote `count` items into the buffer; names point inside it.
            let it = unsafe { &*items.add(i) };
            let st = it.FmtValue.CStatus;
            if st != PDH_CSTATUS_VALID_DATA && st != PDH_CSTATUS_NEW_DATA {
                continue;
            }
            let name = unsafe { it.szName.to_string() }.unwrap_or_default();
            // SAFETY: the union member read matches the requested format.
            let v = unsafe {
                if fmt == PDH_FMT_LARGE {
                    it.FmtValue.Anonymous.largeValue as f64
                } else {
                    it.FmtValue.Anonymous.doubleValue
                }
            };
            out.push((name, v));
        }
        out
    }
}

struct Static {
    luid: (u32, u32),
    name: String,
    vendor: String,
    driver: String,
    ded_total: u64,
    shared_total: u64,
    integrated: bool,
    dxgi: Option<IDXGIAdapter3>,
    kmt: Option<u32>,
    caps: Option<D3DKMT_ADAPTER_PERFDATACAPS>,
    last_bw: Option<(Instant, u64, u64)>,
}

impl Drop for Static {
    fn drop(&mut self) {
        if let Some(h) = self.kmt {
            // SAFETY: handle came from D3DKMTOpenAdapterFromLuid.
            unsafe {
                let _ = D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER { hAdapter: h });
            }
        }
    }
}

fn kmt_query<T: Default>(h: u32, kind: KMTQUERYADAPTERINFOTYPE, mut data: T) -> Option<T> {
    let mut q = D3DKMT_QUERYADAPTERINFO {
        hAdapter: h,
        Type: kind,
        pPrivateDriverData: &mut data as *mut T as *mut _,
        PrivateDriverDataSize: std::mem::size_of::<T>() as u32,
    };
    // SAFETY: the private data pointer and size describe `data`, which outlives the call.
    let st = unsafe { D3DKMTQueryAdapterInfo(&mut q) };
    (st.0 >= 0).then_some(data)
}

fn luid_key(l: LUID) -> (u32, u32) {
    (l.HighPart as u32, l.LowPart)
}

fn enumerate() -> Vec<Static> {
    let mut out = Vec::new();
    // SAFETY: standard DXGI factory creation; COM objects are released on drop.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return out;
    };
    for i in 0.. {
        // SAFETY: enumeration stops at the first error (DXGI_ERROR_NOT_FOUND).
        let Ok(adapter): Result<IDXGIAdapter1, _> = (unsafe { factory.EnumAdapters1(i) }) else {
            break;
        };
        // SAFETY: valid adapter interface.
        let Ok(d) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue; // Microsoft Basic Render Driver
        }
        let luid = luid_key(d.AdapterLuid);
        if out.iter().any(|s: &Static| s.luid == luid) {
            continue;
        }
        let name = String::from_utf16_lossy(
            &d.Description[..d.Description.iter().position(|&c| c == 0).unwrap_or(128)],
        );
        // SAFETY: valid adapter; IID pointer is a static.
        let driver = unsafe { adapter.CheckInterfaceSupport(&IDXGIDevice::IID) }
            .map(driver_version)
            .unwrap_or_default();
        let mut open = D3DKMT_OPENADAPTERFROMLUID {
            AdapterLuid: d.AdapterLuid,
            hAdapter: 0,
        };
        // SAFETY: out-struct is live; the handle is closed in Static::drop.
        let kmt = (unsafe { D3DKMTOpenAdapterFromLuid(&mut open) }.0 >= 0).then_some(open.hAdapter);
        let caps = kmt.and_then(|h| {
            kmt_query(
                h,
                KMTQAITYPE_ADAPTERPERFDATA_CAPS,
                D3DKMT_ADAPTER_PERFDATACAPS::default(),
            )
        });
        out.push(Static {
            luid,
            name: name.trim().to_string(),
            vendor: vendor_name(d.VendorId).to_string(),
            driver,
            ded_total: d.DedicatedVideoMemory as u64,
            shared_total: d.SharedSystemMemory as u64,
            integrated: (d.DedicatedVideoMemory as u64) < 512 * 1024 * 1024,
            dxgi: adapter.cast::<IDXGIAdapter3>().ok(),
            kmt,
            caps,
            last_bw: None,
        });
    }
    out
}

pub struct GpuSampler {
    query: Option<Query>,
    adapters: Vec<Static>,
    last: GpuSample,
}

impl Default for GpuSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl GpuSampler {
    pub fn new() -> Self {
        let query = Query::new();
        if query.is_none() {
            crate::diag::warn(
                "GPU: could not open the PDH performance query (GPU counters unavailable)",
            );
        }
        let adapters = enumerate();
        if adapters.is_empty() {
            crate::diag::warn("GPU: DXGI found no hardware adapters");
        }
        for a in &adapters {
            crate::diag::info(&format!(
                "GPU: {} ({}) driver {} dedicated {} MB, kernel handle {}, perf caps {}",
                a.name,
                a.vendor,
                a.driver,
                a.ded_total / 1024 / 1024,
                if a.kmt.is_some() { "ok" } else { "unavailable" },
                if a.caps.is_some() {
                    "ok"
                } else {
                    "unavailable"
                }
            ));
        }
        GpuSampler {
            query,
            adapters,
            last: GpuSample::default(),
        }
    }

    pub fn sample(&mut self, _pids: &[(u32, &str)]) -> GpuSample {
        if self.adapters.is_empty() {
            return GpuSample::default();
        }
        let rebuild = self
            .query
            .as_ref()
            .is_none_or(|q| q.built.elapsed().as_secs() >= REBUILD_SECS);
        if rebuild {
            self.query = Query::new();
            // A fresh query needs one more interval before rate counters are valid.
            let mut s = self.last.clone();
            self.fill_sensors(&mut s);
            return s;
        }
        let q = self.query.as_mut().unwrap();
        // SAFETY: valid query handle.
        unsafe { PdhCollectQueryData(q.q) };
        q.samples += 1;
        let q = self.query.as_ref().unwrap();

        let idx_of = |luid: Option<(u32, u32)>| {
            luid.and_then(|l| self.adapters.iter().position(|a| a.luid == l))
        };

        // engine key (adapter, phys, eng) -> (type, summed util)
        // Engine-type strings below borrow from `engine_items` (no per-instance allocation).
        let engine_items = q.array(q.engine, PDH_FMT_DOUBLE);
        let mut engines: HashMap<(usize, u32, u32), (&str, f64)> = HashMap::new();
        let mut procs: HashMap<u32, GpuProc> = HashMap::new();
        // per (pid, adapter, eng) utilisation
        let mut per_proc_engine: HashMap<(u32, usize, u32), (&str, f64)> = HashMap::new();
        for (name, v) in &engine_items {
            let inst = parse_gpu_instance(name);
            let (Some(ai), Some(eng)) = (idx_of(inst.luid), inst.eng) else {
                continue;
            };
            engines
                .entry((ai, inst.phys, eng))
                .or_insert((inst.engtype, 0.0))
                .1 += v;
            if let Some(pid) = inst.pid {
                per_proc_engine
                    .entry((pid, ai, eng))
                    .or_insert((inst.engtype, 0.0))
                    .1 += v;
            }
        }
        // Each process: its busiest engine; the label is formatted once, only if it's in use.
        let mut best: HashMap<u32, (f32, usize, &str)> = HashMap::new();
        for ((pid, ai, _), (ty, v)) in per_proc_engine {
            let v = v.min(100.0) as f32;
            let e = best.entry(pid).or_insert((-1.0, ai, ty));
            if v > e.0 {
                *e = (v, ai, ty);
            }
        }
        for (pid, (v, ai, ty)) in best {
            let p = procs.entry(pid).or_default();
            p.util = v.max(0.0);
            if v > 0.0 {
                p.engine = format!("GPU {ai} - {}", engine_label(ty));
            }
        }
        for (name, v) in q.array(q.proc_ded, PDH_FMT_LARGE) {
            if let Some(pid) = parse_gpu_instance(&name).pid {
                procs.entry(pid).or_default().dedicated += v as u64;
            }
        }
        for (name, v) in q.array(q.proc_shared, PDH_FMT_LARGE) {
            if let Some(pid) = parse_gpu_instance(&name).pid {
                procs.entry(pid).or_default().shared += v as u64;
            }
        }
        procs.remove(&0);

        let mut ded: HashMap<usize, u64> = HashMap::new();
        let mut shared: HashMap<usize, u64> = HashMap::new();
        for (name, v) in q.array(q.ad_ded, PDH_FMT_LARGE) {
            if let Some(ai) = idx_of(parse_gpu_instance(&name).luid) {
                *ded.entry(ai).or_default() += v as u64;
            }
        }
        for (name, v) in q.array(q.ad_shared, PDH_FMT_LARGE) {
            if let Some(ai) = idx_of(parse_gpu_instance(&name).luid) {
                *shared.entry(ai).or_default() += v as u64;
            }
        }

        let mut adapters = Vec::new();
        for (ai, st) in self.adapters.iter().enumerate() {
            let mut by_type: HashMap<&str, f32> = HashMap::new();
            for ((a, _, _), (ty, v)) in &engines {
                if *a == ai {
                    let e = by_type.entry(*ty).or_default();
                    *e = e.max((*v as f32).min(100.0));
                }
            }
            let mut list: Vec<GpuEngine> = by_type
                .into_iter()
                .map(|(n, u)| GpuEngine {
                    name: engine_label(n),
                    util: u,
                })
                .collect();
            list.sort_by(|a, b| {
                engine_rank(&a.name)
                    .cmp(&engine_rank(&b.name))
                    .then(a.name.cmp(&b.name))
            });
            let util = list.iter().map(|e| e.util).fold(0.0, f32::max);
            adapters.push(GpuAdapter {
                index: ai,
                name: st.name.clone(),
                vendor: st.vendor.clone(),
                driver: st.driver.clone(),
                integrated: st.integrated,
                util,
                engines: list,
                ded_used: ded.get(&ai).copied().unwrap_or(0),
                ded_total: st.ded_total,
                shared_used: shared.get(&ai).copied().unwrap_or(0),
                shared_total: st.shared_total,
                ..Default::default()
            });
        }
        let mut sample = GpuSample { adapters, procs };
        self.fill_sensors(&mut sample);
        self.last = sample.clone();
        sample
    }

    fn fill_sensors(&mut self, s: &mut GpuSample) {
        for (ai, st) in self.adapters.iter_mut().enumerate() {
            let Some(a) = s.adapters.get_mut(ai) else {
                continue;
            };
            if let Some(dx) = &st.dxgi {
                let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
                // SAFETY: valid adapter; out-struct is live.
                if unsafe { dx.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info) }
                    .is_ok()
                {
                    a.budget = info.Budget;
                }
            }
            let Some(h) = st.kmt else { continue };
            if let Some(p) = kmt_query(
                h,
                KMTQAITYPE_ADAPTERPERFDATA,
                D3DKMT_ADAPTER_PERFDATA::default(),
            ) {
                if p.Temperature > 0 {
                    a.temp_c = Some(p.Temperature as f32 / 10.0);
                }
                if p.FanRPM > 0 {
                    a.fan_rpm = Some(p.FanRPM);
                }
                if p.Power > 0 {
                    a.power_pct = Some(p.Power as f32 / 10.0);
                }
                if p.MemoryFrequency > 0 {
                    a.mem_mhz = Some(p.MemoryFrequency as f32 / 1e6);
                }
                if p.MaxMemoryFrequency > 0 {
                    a.mem_max_mhz = Some(p.MaxMemoryFrequency as f32 / 1e6);
                }
                let now = Instant::now();
                if let Some((t, mb, pb)) = st.last_bw {
                    let dt = now.duration_since(t).as_secs_f64().max(0.001);
                    if p.MemoryBandwidth >= mb && p.MemoryBandwidth > 0 {
                        a.mem_bandwidth = Some((p.MemoryBandwidth - mb) as f64 / dt);
                    }
                    if p.PCIEBandwidth >= pb && p.PCIEBandwidth > 0 {
                        a.pcie_bandwidth = Some((p.PCIEBandwidth - pb) as f64 / dt);
                    }
                }
                st.last_bw = Some((now, p.MemoryBandwidth, p.PCIEBandwidth));
            }
            if let Some(c) = &st.caps {
                if c.TemperatureMax > 0 {
                    a.temp_max_c = Some(c.TemperatureMax as f32 / 10.0);
                }
                if c.MaxFanRPM > 0 {
                    a.fan_max_rpm = Some(c.MaxFanRPM);
                }
            }
            // Core clock / voltage from node 0 (normally the 3D engine).
            if let Some(n) = kmt_query(h, KMTQAITYPE_NODEPERFDATA, D3DKMT_NODE_PERFDATA::default())
            {
                if n.Frequency > 0 {
                    a.core_mhz = Some(n.Frequency as f32 / 1e6);
                }
                if n.MaxFrequency > 0 {
                    a.core_max_mhz = Some(n.MaxFrequency as f32 / 1e6);
                }
                if n.Voltage > 0 {
                    a.voltage_mv = Some(n.Voltage);
                }
            }
        }
    }
}

/// Display order: the engines people care about first.
fn engine_rank(name: &str) -> u8 {
    match name {
        "3D" => 0,
        "Compute 0" | "Compute" => 1,
        "Copy" => 2,
        "Video Decode" => 3,
        "Video Encode" => 4,
        "Video Processing" => 5,
        _ => 9,
    }
}
