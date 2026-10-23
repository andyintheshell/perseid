//! Allocator heap tracking for the status bar.
//!
//! Tracks the bytes currently held by the process allocator, plus a rolling
//! history for the memory window graph. The counter reflects requested bytes,
//! not allocator overhead.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// How often the UI refreshes its heap sample.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
/// Number of samples kept for the memory window graph (three minutes).
pub const SAMPLE_HISTORY: usize = 180;

static HEAP_IN_USE: AtomicUsize = AtomicUsize::new(0);
static HEAP_PEAK: AtomicUsize = AtomicUsize::new(0);

/// A `System` allocator wrapper that tracks live and peak heap bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct TrackingAllocator;

impl TrackingAllocator {
    fn grew(&self, bytes: usize) {
        let current = HEAP_IN_USE.fetch_add(bytes, Ordering::Relaxed) + bytes;
        HEAP_PEAK.fetch_max(current, Ordering::Relaxed);
    }

    fn shrank(&self, bytes: usize) {
        let mut current = HEAP_IN_USE.load(Ordering::Relaxed);
        loop {
            let next = current.saturating_sub(bytes);
            match HEAP_IN_USE.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }
}

// SAFETY: every method forwards to the system allocator with the caller's
// pointer and layout, and only updates two atomics afterwards. Pointer
// validity and layout rules are unchanged.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            self.grew(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            self.grew(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        self.shrank(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            if new_size >= layout.size() {
                self.grew(new_size - layout.size());
            } else {
                self.shrank(layout.size() - new_size);
            }
        }
        new_ptr
    }
}

/// Bytes currently held by the process allocator.
pub fn heap_in_use() -> u64 {
    HEAP_IN_USE.load(Ordering::Relaxed) as u64
}

/// Physical memory installed on this machine, when the platform exposes it.
///
/// This is intentionally a one-shot startup query, not the free-memory figure:
/// the history budget should stay stable for the whole session.
#[cfg(unix)]
pub fn physical_memory_bytes() -> Option<u64> {
    // SAFETY: `sysconf` only reads process/platform limits and does not retain
    // pointers.
    let pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if pages > 0 && page_size > 0 {
        Some((pages as u64).saturating_mul(page_size as u64))
    } else {
        None
    }
}

#[cfg(windows)]
pub fn physical_memory_bytes() -> Option<u64> {
    let mut kilobytes = 0u64;
    // SAFETY: the pointer is valid for the duration of the call.
    let ok = unsafe {
        windows_sys::Win32::System::SystemInformation::GetPhysicallyInstalledSystemMemory(
            &mut kilobytes,
        )
    };
    (ok != 0 && kilobytes > 0).then_some(kilobytes.saturating_mul(1024))
}

#[cfg(not(any(unix, windows)))]
pub fn physical_memory_bytes() -> Option<u64> {
    None
}

/// Peak bytes held by the process allocator since startup.
pub fn heap_peak() -> u64 {
    HEAP_PEAK.load(Ordering::Relaxed) as u64
}

/// A rolling sampler for the allocator's live heap bytes.
#[derive(Debug)]
pub struct MemoryMeter {
    samples: VecDeque<u64>,
    last_sample: Instant,
}

impl MemoryMeter {
    pub fn new() -> Self {
        let mut samples = VecDeque::with_capacity(SAMPLE_HISTORY);
        samples.push_back(heap_in_use());
        Self {
            samples,
            last_sample: Instant::now(),
        }
    }

    /// Refresh the sample if the interval has elapsed.
    pub fn sample_if_due(&mut self) {
        if self.last_sample.elapsed() < SAMPLE_INTERVAL {
            return;
        }
        self.last_sample = Instant::now();
        if self.samples.len() == SAMPLE_HISTORY {
            self.samples.pop_front();
        }
        self.samples.push_back(heap_in_use());
    }

    /// Oldest first. Values are heap bytes in use.
    pub fn samples(&self) -> &VecDeque<u64> {
        &self.samples
    }

    /// How long until the next automatic sample should be taken.
    pub fn time_until_next_sample(&self) -> Duration {
        SAMPLE_INTERVAL.saturating_sub(self.last_sample.elapsed())
    }

    /// Fill the graph with a deterministic, realistic-looking history so the
    /// screenshot tool does not have to wait three minutes. Only used by the
    /// `screenshots` feature; the live meter keeps sampling normally.
    #[cfg(feature = "screenshots")]
    pub fn seed_presentation_samples(&mut self) {
        let current = heap_in_use().max(1) as f32;
        let peak = heap_peak().max(heap_in_use()).max(1) as f32;
        self.samples.clear();
        for index in 0..SAMPLE_HISTORY {
            let t = index as f32 / (SAMPLE_HISTORY - 1) as f32;
            let step = 0.14 * smoothstep((t - 0.12) / 0.24);
            let bump = 0.045 * (1.0 - ((t - 0.72) / 0.07).abs()).max(0.0);
            let wobble = 0.003 * (index as f32 * 0.83).sin();
            let sample = (current * (0.86 + step + bump) * (1.0 + wobble)).min(peak);
            self.samples.push_back(sample.max(1.0) as u64);
        }
        self.last_sample = Instant::now();
    }
}

/// Keep the memory window from reporting the screenshot resizer's temporary
/// buffers as the application's peak. Only used by the `screenshots` feature.
#[cfg(feature = "screenshots")]
pub fn reset_peak_for_screenshot() {
    HEAP_PEAK.store(HEAP_IN_USE.load(Ordering::Relaxed), Ordering::Relaxed);
}

#[cfg(feature = "screenshots")]
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Default for MemoryMeter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// The heap counters are process-wide, so the tests that pin them
    /// serialize on this lock even though `cargo test` runs them on parallel
    /// threads. The lib test binary uses the system allocator (only the
    /// `perseid` binary installs [`TrackingAllocator`]), so nothing else
    /// changes the counters while these tests run.
    static COUNTER_LOCK: Mutex<()> = Mutex::new(());

    fn lock_counters() -> MutexGuard<'static, ()> {
        COUNTER_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Restores the live and peak counters when dropped, so pinning them for
    /// a test does not disturb the rest of the suite.
    struct CounterSnapshot {
        in_use: usize,
        peak: usize,
    }

    impl CounterSnapshot {
        fn take() -> Self {
            Self {
                in_use: HEAP_IN_USE.load(Ordering::Relaxed),
                peak: HEAP_PEAK.load(Ordering::Relaxed),
            }
        }
    }

    impl Drop for CounterSnapshot {
        fn drop(&mut self) {
            HEAP_IN_USE.store(self.in_use, Ordering::Relaxed);
            HEAP_PEAK.store(self.peak, Ordering::Relaxed);
        }
    }

    fn past_sample_deadline(meter: &mut MemoryMeter) {
        meter.last_sample = Instant::now()
            .checked_sub(SAMPLE_INTERVAL + Duration::from_millis(1))
            .expect("the process has been running for over a second");
    }

    #[test]
    fn sample_history_covers_three_minutes() {
        assert_eq!(
            SAMPLE_INTERVAL * SAMPLE_HISTORY as u32,
            Duration::from_secs(180)
        );
    }

    #[cfg(unix)]
    #[test]
    fn physical_memory_is_reported_on_unix() {
        let bytes = physical_memory_bytes().expect("sysconf reports physical memory");
        assert!(bytes > 0);
    }

    #[test]
    fn new_meter_starts_with_one_live_sample() {
        let _guard = lock_counters();
        let _snapshot = CounterSnapshot::take();
        HEAP_IN_USE.store(1234, Ordering::Relaxed);

        let meter = MemoryMeter::new();
        assert_eq!(meter.samples().len(), 1);
        assert_eq!(meter.samples()[0], 1234);
        assert!(meter.time_until_next_sample() <= SAMPLE_INTERVAL);

        assert_eq!(MemoryMeter::default().samples().len(), 1);
    }

    #[test]
    fn sample_if_due_waits_for_the_interval() {
        let _guard = lock_counters();
        let mut meter = MemoryMeter::new();
        let before = meter.samples().len();

        meter.last_sample = Instant::now();
        meter.sample_if_due();

        assert_eq!(meter.samples().len(), before);
        assert!(meter.time_until_next_sample() > Duration::ZERO);
    }

    #[test]
    fn a_meter_past_its_deadline_reports_no_time_left() {
        let _guard = lock_counters();
        let mut meter = MemoryMeter::new();
        past_sample_deadline(&mut meter);
        assert_eq!(meter.time_until_next_sample(), Duration::ZERO);
    }

    #[test]
    fn sample_if_due_appends_at_the_end_and_evicts_at_capacity() {
        let _guard = lock_counters();
        let _snapshot = CounterSnapshot::take();
        let mut meter = MemoryMeter::new();
        meter.samples.clear();
        meter.samples.extend(0..SAMPLE_HISTORY as u64);
        past_sample_deadline(&mut meter);
        HEAP_IN_USE.store(999, Ordering::Relaxed);

        meter.sample_if_due();

        assert_eq!(meter.samples().len(), SAMPLE_HISTORY);
        assert_eq!(*meter.samples().front().unwrap(), 1);
        assert_eq!(*meter.samples().back().unwrap(), 999);
    }

    #[test]
    fn grew_and_shrank_track_live_and_peak_bytes() {
        let _guard = lock_counters();
        let _snapshot = CounterSnapshot::take();
        HEAP_IN_USE.store(0, Ordering::Relaxed);
        HEAP_PEAK.store(0, Ordering::Relaxed);
        let allocator = TrackingAllocator;

        allocator.grew(100);
        assert_eq!(heap_in_use(), 100);
        assert_eq!(heap_peak(), 100);

        allocator.shrank(40);
        assert_eq!(heap_in_use(), 60);
        assert_eq!(heap_peak(), 100);

        allocator.grew(5);
        assert_eq!(heap_in_use(), 65);
        assert_eq!(heap_peak(), 100);
    }

    #[test]
    fn shrank_saturates_at_zero() {
        let _guard = lock_counters();
        let _snapshot = CounterSnapshot::take();
        HEAP_IN_USE.store(4, Ordering::Relaxed);
        HEAP_PEAK.store(4, Ordering::Relaxed);

        TrackingAllocator.shrank(10);

        assert_eq!(heap_in_use(), 0);
        assert_eq!(heap_peak(), 4);
    }

    #[test]
    fn alloc_and_dealloc_are_forwarded_and_tracked() {
        let _guard = lock_counters();
        let _snapshot = CounterSnapshot::take();
        HEAP_IN_USE.store(0, Ordering::Relaxed);
        HEAP_PEAK.store(0, Ordering::Relaxed);
        let allocator = TrackingAllocator;
        let layout = Layout::from_size_align(64, 8).unwrap();

        // SAFETY: the allocation is freed below with its own layout.
        unsafe {
            let ptr = allocator.alloc(layout);
            assert!(!ptr.is_null());
            assert_eq!(heap_in_use(), 64);

            allocator.dealloc(ptr, layout);
        }
        assert_eq!(heap_in_use(), 0);
        assert_eq!(heap_peak(), 64);
    }

    #[test]
    fn alloc_zeroed_is_tracked_and_zeroed() {
        let _guard = lock_counters();
        let _snapshot = CounterSnapshot::take();
        HEAP_IN_USE.store(0, Ordering::Relaxed);
        let allocator = TrackingAllocator;
        let layout = Layout::from_size_align(128, 8).unwrap();

        // SAFETY: the allocation is freed below with its own layout.
        unsafe {
            let ptr = allocator.alloc_zeroed(layout);
            assert!(!ptr.is_null());
            assert_eq!(heap_in_use(), 128);
            let bytes = std::slice::from_raw_parts(ptr, 128);
            assert!(bytes.iter().all(|&byte| byte == 0));

            allocator.dealloc(ptr, layout);
        }
        assert_eq!(heap_in_use(), 0);
    }

    #[test]
    fn realloc_tracks_the_size_delta_in_both_directions() {
        let _guard = lock_counters();
        let _snapshot = CounterSnapshot::take();
        HEAP_IN_USE.store(0, Ordering::Relaxed);
        HEAP_PEAK.store(0, Ordering::Relaxed);
        let allocator = TrackingAllocator;
        let small = Layout::from_size_align(64, 8).unwrap();
        let medium = Layout::from_size_align(256, 8).unwrap();
        let tiny = Layout::from_size_align(32, 8).unwrap();

        // SAFETY: every pointer is reallocated or freed with its own layout.
        unsafe {
            let ptr = allocator.alloc(small);
            assert!(!ptr.is_null());

            let bigger = allocator.realloc(ptr, small, 256);
            assert!(!bigger.is_null());
            assert_eq!(heap_in_use(), 256);

            let smaller = allocator.realloc(bigger, medium, 32);
            assert!(!smaller.is_null());
            assert_eq!(heap_in_use(), 32);
            assert_eq!(heap_peak(), 256);

            allocator.dealloc(smaller, tiny);
        }
        assert_eq!(heap_in_use(), 0);
    }

    #[test]
    fn failed_allocations_do_not_change_the_counters() {
        let _guard = lock_counters();
        let _snapshot = CounterSnapshot::take();
        HEAP_IN_USE.store(0, Ordering::Relaxed);
        let layout = Layout::from_size_align(1 << 62, 8).unwrap();

        // SAFETY: a failed allocation returns null and owns no memory.
        let ptr = unsafe { TrackingAllocator.alloc(layout) };

        assert!(ptr.is_null());
        assert_eq!(heap_in_use(), 0);
    }

    #[cfg(feature = "screenshots")]
    mod presentation {
        use super::*;

        #[test]
        fn smoothstep_saturates_and_hits_its_midpoint() {
            assert_eq!(smoothstep(-1.0), 0.0);
            assert_eq!(smoothstep(0.0), 0.0);
            assert_eq!(smoothstep(0.5), 0.5);
            assert_eq!(smoothstep(1.0), 1.0);
            assert_eq!(smoothstep(2.0), 1.0);
        }

        #[test]
        fn seeded_samples_fill_the_graph_within_the_observed_peak() {
            let _guard = lock_counters();
            let _snapshot = CounterSnapshot::take();
            HEAP_IN_USE.store(1000, Ordering::Relaxed);
            HEAP_PEAK.store(2000, Ordering::Relaxed);

            let mut meter = MemoryMeter::new();
            meter.seed_presentation_samples();

            assert_eq!(meter.samples().len(), SAMPLE_HISTORY);
            assert!(
                meter
                    .samples()
                    .iter()
                    .all(|&sample| (1..=2000).contains(&sample))
            );
            assert!(meter.time_until_next_sample() <= SAMPLE_INTERVAL);
        }

        #[test]
        fn reset_peak_aligns_the_peak_with_the_live_heap() {
            let _guard = lock_counters();
            let _snapshot = CounterSnapshot::take();
            HEAP_IN_USE.store(42, Ordering::Relaxed);
            HEAP_PEAK.store(999, Ordering::Relaxed);

            reset_peak_for_screenshot();

            assert_eq!(heap_peak(), 42);
        }
    }
}
