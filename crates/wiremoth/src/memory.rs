//! Lightweight process-memory sampling for the status bar.
//!
//! Linux exposes both current and peak resident set size in
//! `/proc/self/status`. Other platforms report the meter as unavailable rather
//! than presenting a misleading estimate.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How often the UI refreshes its memory sample.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
/// Number of samples kept for the memory window graph (three minutes).
pub const SAMPLE_HISTORY: usize = 180;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemorySample {
    /// Current resident set size in bytes.
    pub rss: u64,
    /// Peak resident set size in bytes.
    pub peak_rss: u64,
}

/// A rolling sampler for the process's resident memory.
#[derive(Debug)]
pub struct MemoryMeter {
    current: Option<MemorySample>,
    samples: VecDeque<u64>,
    last_sample: Instant,
    supported: bool,
}

impl MemoryMeter {
    pub fn new() -> Self {
        let supported = cfg!(target_os = "linux");
        let current = sample_process_memory();
        let mut samples = VecDeque::with_capacity(SAMPLE_HISTORY);
        if let Some(sample) = current {
            samples.push_back(sample.rss);
        }
        Self {
            current,
            samples,
            last_sample: Instant::now(),
            supported,
        }
    }

    /// Refresh the sample if the interval has elapsed.
    pub fn sample_if_due(&mut self) {
        if self.last_sample.elapsed() < SAMPLE_INTERVAL {
            return;
        }
        self.last_sample = Instant::now();
        let Some(sample) = sample_process_memory() else {
            return;
        };
        self.current = Some(sample);
        if self.samples.len() == SAMPLE_HISTORY {
            self.samples.pop_front();
        }
        self.samples.push_back(sample.rss);
    }

    pub fn current(&self) -> Option<MemorySample> {
        self.current
    }

    /// Oldest first. Values are RSS in bytes.
    pub fn samples(&self) -> &VecDeque<u64> {
        &self.samples
    }

    pub fn supported(&self) -> bool {
        self.supported
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

#[cfg(target_os = "linux")]
fn sample_process_memory() -> Option<MemorySample> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    parse_linux_status(&status)
}

#[cfg(not(target_os = "linux"))]
fn sample_process_memory() -> Option<MemorySample> {
    None
}

fn parse_linux_status(status: &str) -> Option<MemorySample> {
    let mut rss = None;
    let mut peak = None;

    for line in status.lines() {
        if let Some(value) = line.strip_prefix("VmRSS:") {
            rss = parse_kib(value);
        } else if let Some(value) = line.strip_prefix("VmHWM:") {
            peak = parse_kib(value);
        }
        if rss.is_some() && peak.is_some() {
            break;
        }
    }

    let rss = rss?;
    Some(MemorySample {
        rss,
        peak_rss: peak.unwrap_or(rss),
    })
}

fn parse_kib(value: &str) -> Option<u64> {
    let kib = value.split_whitespace().next()?.parse::<u64>().ok()?;
    Some(kib.saturating_mul(1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_and_peak_rss() {
        let status = "\
Name:\twiremoth
VmPeak:\t  987654 kB
VmHWM:\t    4096 kB
VmRSS:\t    3072 kB
Threads:\t4
";
        assert_eq!(
            parse_linux_status(status),
            Some(MemorySample {
                rss: 3 * 1024 * 1024,
                peak_rss: 4 * 1024 * 1024,
            })
        );
    }

    #[test]
    fn falls_back_to_rss_when_peak_is_missing() {
        let status = "VmRSS:\t 2048 kB\n";
        assert_eq!(
            parse_linux_status(status),
            Some(MemorySample {
                rss: 2 * 1024 * 1024,
                peak_rss: 2 * 1024 * 1024,
            })
        );
    }

    #[test]
    fn requires_a_current_rss_value() {
        assert_eq!(parse_linux_status("VmHWM:\t4096 kB\n"), None);
        assert_eq!(parse_linux_status("VmRSS:\tnot-a-number kB\n"), None);
    }
}
