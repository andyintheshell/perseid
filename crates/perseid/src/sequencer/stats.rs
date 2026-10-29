//! Statistical primitives used by the Sequencer.
//!
//! The tests here are deliberately self-contained: the desktop app already
//! depends on `flate2`, but it does not need a general statistics crate for a
//! handful of well-known distributions. Each function documents the
//! approximation it uses so the result screen can be honest about the limits
//! of a small sample.

use std::f64::consts::{EULER_GAMMA, FRAC_1_SQRT_2, LN_2, PI};
use std::io::Write;

/// Standard normal cumulative distribution.
pub(super) fn normal_cdf(x: f64) -> f64 {
    0.5 * (1.0 + erf(x * FRAC_1_SQRT_2))
}

/// Abramowitz and Stegun 7.1.26, accurate to about 1.5e-7.
fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    sign * y
}

/// Two-sided binomial p-value for `k` successes in `n` fair trials.
///
/// Exact summation is used for the sample sizes common in character-level
/// tests; larger bit streams use the normal approximation with a continuity
/// correction.
pub(super) fn binomial_two_sided(k: usize, n: usize, p: f64) -> f64 {
    if n == 0 {
        return 1.0;
    }
    if p <= 0.0 {
        return if k == 0 { 1.0 } else { 0.0 };
    }
    if p >= 1.0 {
        return if k == n { 1.0 } else { 0.0 };
    }

    if n <= 200 {
        let log_p = p.ln();
        let log_q = (1.0 - p).ln();
        let mut logs = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let log_pmf = ln_binomial(n, i) + i as f64 * log_p + (n - i) as f64 * log_q;
            logs.push(log_pmf);
        }
        let observed = logs[k];
        let mut total = 0.0;
        for log_pmf in logs {
            if log_pmf <= observed + 1e-12 {
                total += log_pmf.exp();
            }
        }
        return total.clamp(0.0, 1.0);
    }

    let mean = n as f64 * p;
    let variance = n as f64 * p * (1.0 - p);
    if variance <= 0.0 {
        return if (k as f64 - mean).abs() < 0.5 {
            1.0
        } else {
            0.0
        };
    }
    let distance = ((k as f64 - mean).abs() - 0.5).max(0.0);
    let z = distance / variance.sqrt();
    (2.0 * (1.0 - normal_cdf(z))).clamp(0.0, 1.0)
}

fn ln_binomial(n: usize, k: usize) -> f64 {
    ln_gamma(n as f64 + 1.0) - ln_gamma(k as f64 + 1.0) - ln_gamma((n - k) as f64 + 1.0)
}

/// Lanczos approximation of `ln Γ(x)` for positive `x`.
fn ln_gamma(x: f64) -> f64 {
    const COEFFICIENTS: [f64; 6] = [
        76.180_091_729_471_46,
        -86.505_320_329_416_77,
        24.014_098_240_830_91,
        -1.231_739_572_450_155,
        0.000_120_865_097_386_617_9,
        -0.000_000_539_523_938_495_3,
    ];
    let mut y = x;
    let temp = x + 5.5;
    let temp = temp - (x + 0.5) * temp.ln();
    let mut series = 1.000_000_000_190_015;
    for coefficient in COEFFICIENTS {
        y += 1.0;
        series += coefficient / y;
    }
    -temp + (2.506_628_274_631_000_5 * series / x).ln()
}

/// Upper tail of the chi-square distribution: `P(X > x)`.
pub(super) fn chi_square_sf(x: f64, df: usize) -> f64 {
    if df == 0 {
        return 1.0;
    }
    if !x.is_finite() || x <= 0.0 {
        return 1.0;
    }
    gamma_q(df as f64 / 2.0, x / 2.0).clamp(0.0, 1.0)
}

fn gamma_q(a: f64, x: f64) -> f64 {
    if x < a + 1.0 {
        1.0 - gamma_series(a, x)
    } else {
        gamma_continued_fraction(a, x)
    }
}

fn gamma_series(a: f64, x: f64) -> f64 {
    let mut sum = 1.0 / a;
    let mut term = sum;
    let mut denominator = a;
    for _ in 0..1_000 {
        denominator += 1.0;
        term *= x / denominator;
        sum += term;
        if term.abs() < sum.abs() * 1e-14 {
            break;
        }
    }
    (sum * (-x + a * x.ln() - ln_gamma(a)).exp()).clamp(0.0, 1.0)
}

fn gamma_continued_fraction(a: f64, x: f64) -> f64 {
    const TINY: f64 = 1e-300;
    let mut b = x + 1.0 - a;
    let mut c = 1.0 / TINY;
    let mut d = 1.0 / b;
    let mut h = d;
    for index in 1..=1_000 {
        let index = index as f64;
        let a_n = -index * (index - a);
        b += 2.0;
        d = a_n * d + b;
        if d.abs() < TINY {
            d = TINY;
        }
        c = b + a_n / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < 1e-14 {
            break;
        }
    }
    ((-x + a * x.ln() - ln_gamma(a)).exp() * h).clamp(0.0, 1.0)
}

/// Wald–Wolfowitz runs test on a binary sequence.
pub(super) fn runs_test(bits: &[bool]) -> Option<f64> {
    let n = bits.len();
    if n < 2 {
        return None;
    }
    let ones = bits.iter().filter(|bit| **bit).count();
    let zeros = n - ones;
    if ones == 0 || zeros == 0 {
        return Some(0.0);
    }
    let runs = 1 + bits.windows(2).filter(|pair| pair[0] != pair[1]).count();
    let n = n as f64;
    let ones = ones as f64;
    let zeros = zeros as f64;
    let expected = 1.0 + 2.0 * ones * zeros / n;
    let variance = 2.0 * ones * zeros * (2.0 * ones * zeros - n) / (n * n * (n - 1.0));
    if variance <= 0.0 {
        return Some(0.0);
    }
    let z = ((runs as f64 - expected).abs() - 0.5).max(0.0) / variance.sqrt();
    Some((2.0 * (1.0 - normal_cdf(z))).clamp(0.0, 1.0))
}

/// Lag-one autocorrelation test for a binary sequence.
pub(super) fn lag_one_autocorrelation(bits: &[bool]) -> Option<f64> {
    let n = bits.len();
    if n < 3 {
        return None;
    }
    let mean = bits.iter().filter(|bit| **bit).count() as f64 / n as f64;
    let mut numerator = 0.0;
    let mut denominator = 0.0;
    for index in 0..n {
        let value = if bits[index] { 1.0 } else { 0.0 } - mean;
        denominator += value * value;
        if index + 1 < n {
            let next = if bits[index + 1] { 1.0 } else { 0.0 } - mean;
            numerator += value * next;
        }
    }
    if denominator == 0.0 {
        return Some(0.0);
    }
    let r = numerator / denominator;
    let z = r * (n as f64).sqrt();
    Some((2.0 * (1.0 - normal_cdf(z.abs()))).clamp(0.0, 1.0))
}

/// Length of the longest run of ones.
pub(super) fn longest_run_of_ones(bits: &[bool]) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for bit in bits {
        if *bit {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

/// Approximate two-sided p-value for the longest run of ones.
///
/// The mean and variance follow the standard extreme-value approximation for
/// the longest head run. It is intentionally presented as an approximation in
/// the result screen.
pub(super) fn longest_run_p_value(longest: usize, n: usize) -> Option<f64> {
    if n < 10 {
        return None;
    }
    if longest == 0 {
        return Some(0.0);
    }
    let mean = (n as f64).log2() + EULER_GAMMA / LN_2 - 0.5;
    let variance = PI * PI / (6.0 * LN_2 * LN_2) + 1.0 / 12.0;
    let z = (longest as f64 - mean) / variance.sqrt();
    Some((2.0 * (1.0 - normal_cdf(z.abs()))).clamp(0.0, 1.0))
}

/// Poker test over non-overlapping 4-bit groups.
///
/// Returns the p-value and the number of groups, or `None` when there are not
/// enough groups for the chi-square approximation.
pub(super) fn poker_test(bits: &[bool]) -> Option<(f64, usize)> {
    let groups = bits.len() / 4;
    if groups < 80 {
        return None;
    }
    let mut counts = [0usize; 16];
    for group in bits.as_chunks::<4>().0 {
        let mut value = 0usize;
        for bit in group {
            value = (value << 1) | usize::from(*bit);
        }
        counts[value] += 1;
    }
    let expected = groups as f64 / 16.0;
    let statistic = counts
        .iter()
        .map(|count| {
            let difference = *count as f64 - expected;
            difference * difference / expected
        })
        .sum();
    Some((chi_square_sf(statistic, 15), groups))
}

/// Chi-square test of byte-value frequencies.
pub(super) fn byte_frequency_test(bytes: &[u8]) -> Option<(f64, usize)> {
    if bytes.len() < 5 * 256 {
        return None;
    }
    let mut counts = [0usize; 256];
    for byte in bytes {
        counts[*byte as usize] += 1;
    }
    let expected = bytes.len() as f64 / 256.0;
    let statistic = counts
        .iter()
        .map(|count| {
            let difference = *count as f64 - expected;
            difference * difference / expected
        })
        .sum();
    Some((chi_square_sf(statistic, 255), bytes.len()))
}

/// Pairwise correlation across short lags, with a Bonferroni adjustment.
///
/// Returns `(minimum raw p-value, adjusted p-value, lag)`.
pub(super) fn pairwise_correlation_test(bits: &[bool]) -> Option<(f64, f64, usize)> {
    let n = bits.len();
    if n < 100 {
        return None;
    }
    let max_lag = 16.min(n / 50).max(1);
    let mut best: Option<(f64, usize)> = None;
    for lag in 1..=max_lag {
        let length = n - lag;
        let mut sum_x = 0.0;
        let mut sum_y = 0.0;
        for index in 0..length {
            sum_x += if bits[index] { 1.0 } else { 0.0 };
            sum_y += if bits[index + lag] { 1.0 } else { 0.0 };
        }
        let mean_x = sum_x / length as f64;
        let mean_y = sum_y / length as f64;
        let mut covariance = 0.0;
        let mut variance_x = 0.0;
        let mut variance_y = 0.0;
        for index in 0..length {
            let x = if bits[index] { 1.0 } else { 0.0 } - mean_x;
            let y = if bits[index + lag] { 1.0 } else { 0.0 } - mean_y;
            covariance += x * y;
            variance_x += x * x;
            variance_y += y * y;
        }
        let denominator = (variance_x * variance_y).sqrt();
        let correlation = if denominator == 0.0 {
            0.0
        } else {
            covariance / denominator
        };
        let z = correlation * (length as f64).sqrt();
        let p = (2.0 * (1.0 - normal_cdf(z.abs()))).clamp(0.0, 1.0);
        if best.is_none_or(|(best_p, _)| p < best_p) {
            best = Some((p, lag));
        }
    }
    let (p, lag) = best?;
    let adjusted = (p * max_lag as f64).min(1.0);
    Some((p, adjusted, lag))
}

/// Zlib compression ratio for the concatenated token bytes.
pub(super) fn compression_ratio(bytes: &[u8]) -> Option<f64> {
    if bytes.is_empty() {
        return None;
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).ok()?;
    let compressed = encoder.finish().ok()?;
    Some(compressed.len() as f64 / bytes.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_and_chi_square_match_known_values() {
        assert!((normal_cdf(0.0) - 0.5).abs() < 1e-9);
        assert!((normal_cdf(1.96) - 0.975).abs() < 1e-3);
        let p = chi_square_sf(3.841, 1);
        assert!((p - 0.05).abs() < 0.002, "p was {p}");
        let p = chi_square_sf(25.0, 15);
        assert!((p - 0.049).abs() < 0.01, "p was {p}");
    }

    #[test]
    fn binomial_is_two_sided_and_symmetric() {
        assert!((binomial_two_sided(50, 100, 0.5) - 1.0).abs() < 1e-9);
        assert!(binomial_two_sided(100, 100, 0.5) < 1e-20);
        assert!((binomial_two_sided(40, 100, 0.5) - 0.0569).abs() < 0.01);
        assert!((binomial_two_sided(60, 100, 0.5) - 0.0569).abs() < 0.01);
        assert!(binomial_two_sided(500, 1_000, 0.5) > 0.9);
    }

    #[test]
    fn runs_test_rejects_alternation_and_constant_sequences() {
        let alternating: Vec<bool> = (0..100).map(|index| index % 2 == 0).collect();
        assert!(runs_test(&alternating).unwrap() < 1e-10);
        assert_eq!(runs_test(&[true; 100]), Some(0.0));
        assert_eq!(runs_test(&[false; 100]), Some(0.0));

        // A pseudo-random sequence should not be rejected at the usual levels.
        let mut state = 0x1234_5678u64;
        let random: Vec<bool> = (0..400)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                (state >> 33) & 1 == 1
            })
            .collect();
        assert!(runs_test(&random).unwrap() > 0.001);
    }

    #[test]
    fn autocorrelation_and_longest_run_are_sane() {
        let alternating: Vec<bool> = (0..100).map(|index| index % 2 == 0).collect();
        assert!(lag_one_autocorrelation(&alternating).unwrap() < 0.001);
        assert_eq!(longest_run_of_ones(&[false, true, true, true, false]), 3);
        assert!(longest_run_p_value(20, 1_000).unwrap() < 0.01);
    }

    #[test]
    fn poker_and_byte_frequency_need_enough_data() {
        assert!(poker_test(&[false; 100]).is_none());
        assert!(byte_frequency_test(&[0; 100]).is_none());

        let mut state = 0x9e37_79b9u64;
        let bits: Vec<bool> = (0..4_000)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                (state >> 33) & 1 == 1
            })
            .collect();
        let (p, groups) = poker_test(&bits).unwrap();
        assert_eq!(groups, 1_000);
        assert!(p > 0.001, "poker p was {p}");
        let mut with_remainder = bits.clone();
        with_remainder.extend([true, false, true]);
        assert_eq!(poker_test(&with_remainder), Some((p, groups)));

        let bytes = vec![0u8; 4_000];
        let (p, count) = byte_frequency_test(&bytes).unwrap();
        assert_eq!(count, 4_000);
        assert!(p < 0.001, "a constant byte stream should fail, p was {p}");
    }

    #[test]
    fn pairwise_correlation_finds_short_periods() {
        let alternating: Vec<bool> = (0..1_000).map(|index| index % 2 == 0).collect();
        let (_, adjusted, lag) = pairwise_correlation_test(&alternating).unwrap();
        assert_eq!(lag, 1);
        assert!(adjusted < 0.001);

        let random: Vec<bool> = (0..1_000).map(|index| index % 7 < 3).collect();
        let (_, adjusted, _) = pairwise_correlation_test(&random).unwrap();
        assert!(adjusted <= 1.0);
    }

    #[test]
    fn compression_ratio_separates_repetition_from_noise() {
        let repeated = vec![b'A'; 10_000];
        assert!(compression_ratio(&repeated).unwrap() < 0.05);
        let mut state = 0x1234_5678_9abc_def0u64;
        let noise: Vec<u8> = (0..10_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect();
        assert!(compression_ratio(&noise).unwrap() > 0.9);
        assert!(compression_ratio(&[]).is_none());
    }

    #[test]
    fn statistical_helpers_handle_degenerate_inputs() {
        assert_eq!(binomial_two_sided(0, 0, 0.5), 1.0);
        assert_eq!(binomial_two_sided(0, 10, 0.0), 1.0);
        assert_eq!(binomial_two_sided(1, 10, 0.0), 0.0);
        assert_eq!(binomial_two_sided(10, 10, 1.0), 1.0);
        assert_eq!(binomial_two_sided(9, 10, 1.0), 0.0);

        assert_eq!(chi_square_sf(1.0, 0), 1.0);
        assert_eq!(chi_square_sf(f64::NAN, 1), 1.0);
        assert_eq!(chi_square_sf(0.0, 1), 1.0);
        assert_eq!(chi_square_sf(f64::INFINITY, 1), 1.0);

        assert_eq!(runs_test(&[]), None);
        assert_eq!(runs_test(&[true]), None);

        assert_eq!(longest_run_p_value(0, 9), None);
        assert_eq!(longest_run_p_value(0, 10), Some(0.0));

        let constant = vec![true; 100];
        let (_, adjusted, lag) = pairwise_correlation_test(&constant).unwrap();
        assert_eq!(lag, 1);
        assert!(adjusted <= 1.0);
    }
}
