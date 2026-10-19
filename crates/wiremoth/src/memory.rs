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
}

impl Default for MemoryMeter {
    fn default() -> Self {
        Self::new()
    }
}
