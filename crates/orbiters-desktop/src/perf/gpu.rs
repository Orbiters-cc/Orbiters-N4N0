//! The graphics card's use over time, from Windows' own performance counters (what Task Manager's GPU columns read):
//! `\GPU Engine(*)\Utilization Percentage`, `\GPU Process Memory(*)\Dedicated Usage` and `\GPU Adapter Memory(*)\Dedicated
//! Usage`. Their instances are named `pid_<pid>_luid_…_engtype_<kind>`: no other process is opened, attached or read, VRChat's
//! included. One query stays open; each [`Gpu::sample`] collects it once, so a percentage is the average since the sample before.
//! Used by the beta's performance figures (`crate::usage::perf`), only while the usage statistics are on.

use std::collections::HashMap;

/// The graphics card over the last interval: percentages of its time (Task Manager's way: per process, its busiest kind of engine;
/// for the whole card, the busiest kind of engine of all processes together), dedicated memory in MB.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuNow {
    pub system: f32,
    pub vram_used_mb: f32,
    /// By process id: (percent, dedicated MB).
    pub by_pid: HashMap<u32, (f32, f32)>,
}

impl GpuNow {
    /// The use of these processes together: percents added (capped at 100), memory added.
    pub fn of(&self, pids: &[u32]) -> (f32, f32) {
        let (mut gpu, mut mb) = (0.0f32, 0.0f32);
        for pid in pids {
            if let Some((g, m)) = self.by_pid.get(pid) {
                gpu += g;
                mb += m;
            }
        }
        (gpu.min(100.0), mb)
    }
}

/// `pid_1234_luid_0x0_0x1_phys_0_eng_0_engtype_3D` → (1234, "3D"); a memory instance has no engine kind.
pub fn instance(name: &str) -> Option<(u32, &str)> {
    let rest = name.strip_prefix("pid_")?;
    let pid = rest.split('_').next()?.parse().ok()?;
    let kind = name.rsplit_once("engtype_").map_or("", |(_, k)| k);
    Some((pid, kind))
}

/// Sums engine readings (instance name, percent) into [`GpuNow`]'s percentages.
pub fn engines(readings: &[(String, f64)]) -> (f32, HashMap<u32, f32>) {
    // (pid, kind) → percent; kind → percent of all processes.
    let mut per: HashMap<(u32, String), f64> = HashMap::new();
    let mut kinds: HashMap<String, f64> = HashMap::new();
    for (name, value) in readings {
        let Some((pid, kind)) = instance(name) else { continue };
        *per.entry((pid, kind.to_string())).or_default() += value;
        *kinds.entry(kind.to_string()).or_default() += value;
    }
    let mut by_pid: HashMap<u32, f32> = HashMap::new();
    for ((pid, _), v) in per {
        let e = by_pid.entry(pid).or_default();
        *e = e.max(v.min(100.0) as f32);
    }
    let system = kinds.values().copied().fold(0.0f64, f64::max).min(100.0) as f32;
    (system, by_pid)
}

#[cfg(windows)]
pub struct Gpu {
    query: windows::Win32::System::Performance::PDH_HQUERY,
    engine: windows::Win32::System::Performance::PDH_HCOUNTER,
    process_memory: windows::Win32::System::Performance::PDH_HCOUNTER,
    adapter_memory: windows::Win32::System::Performance::PDH_HCOUNTER,
    primed: bool,
}

// SAFETY: the query's handles are used by one thread at a time (behind the owner's `&mut`).
#[cfg(windows)]
unsafe impl Send for Gpu {}

#[cfg(windows)]
impl Gpu {
    /// The query, or `None` where Windows has no GPU counters (older drivers, a virtual machine).
    pub fn open() -> Option<Self> {
        use windows::core::w;
        use windows::Win32::System::Performance::*;
        let mut query = PDH_HQUERY::default();
        // SAFETY: out-parameters on the stack; the paths are static texts.
        unsafe {
            if PdhOpenQueryW(windows::core::PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let mut engine = PDH_HCOUNTER::default();
            let mut process_memory = PDH_HCOUNTER::default();
            let mut adapter_memory = PDH_HCOUNTER::default();
            let ok = PdhAddEnglishCounterW(query, w!("\\GPU Engine(*)\\Utilization Percentage"), 0, &mut engine) == 0;
            let _ = PdhAddEnglishCounterW(query, w!("\\GPU Process Memory(*)\\Dedicated Usage"), 0, &mut process_memory);
            let _ = PdhAddEnglishCounterW(query, w!("\\GPU Adapter Memory(*)\\Dedicated Usage"), 0, &mut adapter_memory);
            if !ok {
                PdhCloseQuery(query);
                return None;
            }
            Some(Self { query, engine, process_memory, adapter_memory, primed: false })
        }
    }

    fn read(&self, counter: windows::Win32::System::Performance::PDH_HCOUNTER) -> Vec<(String, f64)> {
        use windows::Win32::System::Performance::*;
        if counter.is_invalid() {
            return Vec::new();
        }
        const NOCAP100: u32 = 0x8000;
        let format = PDH_FMT(PDH_FMT_DOUBLE.0 | NOCAP100);
        let (mut bytes, mut count) = (0u32, 0u32);
        // SAFETY: the first call asks the size; the second fills a buffer of that size, aligned for the items; the names point into it.
        unsafe {
            if PdhGetFormattedCounterArrayW(counter, format, &mut bytes, &mut count, None) != PDH_MORE_DATA || bytes == 0 {
                return Vec::new();
            }
            let mut buffer = vec![0u64; (bytes as usize).div_ceil(8)];
            let items = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            if PdhGetFormattedCounterArrayW(counter, format, &mut bytes, &mut count, Some(items)) != 0 {
                return Vec::new();
            }
            let items = std::slice::from_raw_parts(items, count as usize);
            items
                .iter()
                .filter(|i| i.FmtValue.CStatus == 0 || i.FmtValue.CStatus == 1)
                .map(|i| (i.szName.to_string().unwrap_or_default(), i.FmtValue.Anonymous.doubleValue))
                .collect()
        }
    }

    /// The use since the last call (`None` the first time: a percentage needs two readings).
    pub fn sample(&mut self) -> Option<GpuNow> {
        use windows::Win32::System::Performance::PdhCollectQueryData;
        // SAFETY: an open query.
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return None;
        }
        if !std::mem::replace(&mut self.primed, true) {
            return None;
        }
        let (system, by_pid_gpu) = engines(&self.read(self.engine));
        let mut by_pid: HashMap<u32, (f32, f32)> = by_pid_gpu.into_iter().map(|(p, g)| (p, (g, 0.0))).collect();
        for (name, bytes) in self.read(self.process_memory) {
            if let Some((pid, _)) = instance(&name) {
                by_pid.entry(pid).or_default().1 += (bytes / 1048576.0) as f32;
            }
        }
        let vram_used_mb = self.read(self.adapter_memory).iter().map(|(_, b)| (*b / 1048576.0) as f32).fold(0.0, f32::max);
        Some(GpuNow { system, vram_used_mb, by_pid })
    }
}

#[cfg(windows)]
impl Drop for Gpu {
    fn drop(&mut self) {
        // SAFETY: the query opened in `open`, closed once.
        unsafe {
            windows::Win32::System::Performance::PdhCloseQuery(self.query);
        }
    }
}

/// Elsewhere (macOS) there are no such counters: nothing is sent for the graphics card.
#[cfg(not(windows))]
pub struct Gpu;

#[cfg(not(windows))]
impl Gpu {
    pub fn open() -> Option<Self> {
        None
    }
    pub fn sample(&mut self) -> Option<GpuNow> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_instances_add_up_as_task_manager_shows_them() {
        assert_eq!(instance("pid_1234_luid_0x00000000_0x0000D1E8_phys_0_eng_0_engtype_3D"), Some((1234, "3D")));
        assert_eq!(instance("pid_88_luid_0x00000000_0x0000D1E8_phys_0"), Some((88, "")));
        assert_eq!(instance("luid_0x0_phys_0"), None);
        let r = |n: &str, v: f64| (n.to_string(), v);
        let (system, by_pid) = engines(&[
            r("pid_1_luid_a_phys_0_eng_0_engtype_3D", 40.0),
            r("pid_1_luid_a_phys_0_eng_1_engtype_3D", 5.0),
            r("pid_1_luid_a_phys_0_eng_2_engtype_Copy", 60.0),
            r("pid_2_luid_a_phys_0_eng_0_engtype_3D", 30.0),
        ]);
        // A process: its busiest kind of engine (Copy 60 over 3D 45). The card: the busiest kind of all (3D 75 over Copy 60).
        assert_eq!(by_pid[&1], 60.0);
        assert_eq!(by_pid[&2], 30.0);
        assert_eq!(system, 75.0);
        let now = GpuNow { system, vram_used_mb: 0.0, by_pid: [(1, (60.0, 100.0)), (2, (30.0, 50.0))].into_iter().collect() };
        assert_eq!(now.of(&[1, 2, 3]), (90.0, 150.0));
    }

    /// Windows' counters on this computer: the query opens and gives a second reading (ignored: needs a GPU's counters).
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn the_counters_read_on_this_computer() {
        let mut gpu = Gpu::open().expect("GPU counters");
        assert!(gpu.sample().is_none());
        std::thread::sleep(std::time::Duration::from_secs(1));
        let started = std::time::Instant::now();
        let now = gpu.sample().expect("a second reading");
        eprintln!("GPU sample took {:?}: system {} %, VRAM {} MB, {} processes", started.elapsed(), now.system, now.vram_used_mb, now.by_pid.len());
    }
}
