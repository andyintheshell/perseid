//! Token randomness analysis for the Sequencer tool.
//!
//! The engine works on a sample of token strings. Character-level tests look at
//! the raw token characters; bit-level tests decode the tokens as hexadecimal,
//! Base64, or text bytes and examine the resulting bit positions. Both produce
//! an effective-entropy estimate for each significance level, which the result
//! window charts.
//!
//! The statistics are intentionally bounded and conservative. A sample that is
//! too small for a test is reported as skipped rather than silently treated as
//! random. See [`stats`] for the individual tests and approximations.

mod stats;
mod token;

pub use token::{
    TokenFormat, decode_tokens, detect_format, extract_tokens, parse_tokens, token_length,
};

use std::collections::{BTreeSet, HashMap};

use crate::sequencer::stats::{
    binomial_two_sided, byte_frequency_test, chi_square_sf, compression_ratio,
    lag_one_autocorrelation, longest_run_of_ones, longest_run_p_value, pairwise_correlation_test,
    poker_test, runs_test,
};

/// Hard cap on tokens held by the Sequencer UI.
pub const MAX_TOKENS: usize = 10_000;
/// Hard cap on the number of characters in one token.
pub const MAX_TOKEN_CHARS: usize = 16_384;
/// Tokens used for one analysis. Older tools cap around this size; it keeps the
/// statistical work interactive on a laptop.
pub const MAX_ANALYSIS_TOKENS: usize = 5_000;
/// Bit positions examined by the bit-level tests.
pub const MAX_ANALYSIS_BITS: usize = 4_096;
/// Bytes retained per token for bit-level tests.
pub const MAX_ANALYSIS_BYTES: usize = MAX_ANALYSIS_BITS / 8;
/// Character positions examined by the character-level tests.
pub const MAX_ANALYSIS_CHARS: usize = 1_024;
/// Bits used by the whole-stream tests (monobit, runs, poker, correlation).
pub const MAX_GLOBAL_BITS: usize = 1_000_000;
/// Bytes used by the whole-stream byte-frequency and compression tests.
pub const MAX_GLOBAL_BYTES: usize = 1_000_000;
/// Minimum observations for one bit position to be tested.
pub const MIN_BIT_SAMPLES: usize = 2;
/// A character position is only tested when there are at least this many
/// observations per symbol in the detected alphabet.
pub const CHARACTER_SAMPLE_MULTIPLIER: usize = 10;
/// Transition matrices need more data than simple per-position counts.
pub const TRANSITION_SAMPLE_MULTIPLIER: usize = 5;
/// Character alphabets larger than this are not analyzed.
pub const MAX_CHARACTER_ALPHABET: usize = 128;

/// Burp's five significance levels, from the strictest to the most permissive.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Significance {
    TenPercent,
    #[default]
    OnePercent,
    PointOnePercent,
    PointZeroOnePercent,
    PointZeroZeroOnePercent,
}

impl Significance {
    pub const ALL: [Self; 5] = [
        Self::TenPercent,
        Self::OnePercent,
        Self::PointOnePercent,
        Self::PointZeroOnePercent,
        Self::PointZeroZeroOnePercent,
    ];

    /// Probability threshold below which a test rejects the random hypothesis.
    pub fn alpha(self) -> f64 {
        match self {
            Self::TenPercent => 0.10,
            Self::OnePercent => 0.01,
            Self::PointOnePercent => 0.001,
            Self::PointZeroOnePercent => 0.0001,
            Self::PointZeroZeroOnePercent => 0.00001,
        }
    }

    pub fn percent_label(self) -> &'static str {
        match self {
            Self::TenPercent => "10%",
            Self::OnePercent => "1%",
            Self::PointOnePercent => "0.1%",
            Self::PointZeroOnePercent => "0.01%",
            Self::PointZeroZeroOnePercent => "0.001%",
        }
    }

    pub fn chart_label(self) -> &'static str {
        match self {
            Self::TenPercent => ">10%",
            Self::OnePercent => ">1%",
            Self::PointOnePercent => ">0.1%",
            Self::PointZeroOnePercent => ">0.01%",
            Self::PointZeroZeroOnePercent => ">0.001%",
        }
    }
}

/// Whether character-level tests are attempted automatically, forced, or
/// disabled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CharacterAnalysisMode {
    #[default]
    Auto,
    Always,
    Never,
}

impl CharacterAnalysisMode {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Always, Self::Never];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Always => "Always",
            Self::Never => "Never",
        }
    }
}

/// Options that affect one analysis. Capture-only options live in the app.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnalysisSettings {
    pub format: TokenFormat,
    pub significance: Significance,
    pub character_analysis: CharacterAnalysisMode,
    pub bit_analysis: bool,
}

impl Default for AnalysisSettings {
    fn default() -> Self {
        Self {
            format: TokenFormat::Auto,
            significance: Significance::OnePercent,
            character_analysis: CharacterAnalysisMode::Auto,
            bit_analysis: true,
        }
    }
}

/// Overall randomness rating shown at the top of the result window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rating {
    VeryPoor,
    Poor,
    Fair,
    Good,
    VeryGood,
}

impl Rating {
    pub fn label(self) -> &'static str {
        match self {
            Self::VeryPoor => "very poor",
            Self::Poor => "poor",
            Self::Fair => "fair",
            Self::Good => "good",
            Self::VeryGood => "very good",
        }
    }

    fn from_score(score: f64) -> Self {
        if score >= 0.85 {
            Self::VeryGood
        } else if score >= 0.70 {
            Self::Good
        } else if score >= 0.50 {
            Self::Fair
        } else if score >= 0.30 {
            Self::Poor
        } else {
            Self::VeryPoor
        }
    }
}

/// Reliability depends on sample size, not on the outcome of the tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reliability {
    VeryPoor,
    Poor,
    Fair,
    Good,
    Excellent,
}

impl Reliability {
    pub fn from_tokens(tokens: usize) -> Self {
        if tokens < 200 {
            Self::VeryPoor
        } else if tokens < 500 {
            Self::Poor
        } else if tokens < 1_000 {
            Self::Fair
        } else if tokens < 5_000 {
            Self::Good
        } else {
            Self::Excellent
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::VeryPoor => "very poor",
            Self::Poor => "poor",
            Self::Fair => "fair",
            Self::Good => "good",
            Self::Excellent => "excellent",
        }
    }
}

/// Which test family supplied the headline effective-entropy number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalysisKind {
    Character,
    Bit,
}

impl AnalysisKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Character => "character-level",
            Self::Bit => "bit-level",
        }
    }
}

/// Effective entropy at one significance level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntropyPoint {
    pub significance: Significance,
    pub bits: f64,
}

/// One whole-stream test result.
#[derive(Clone, Debug)]
pub struct TestResult {
    pub name: &'static str,
    pub detail: String,
    pub p_value: Option<f64>,
    pub informational: bool,
    pub skipped: bool,
}

impl TestResult {
    pub fn passed_at(&self, significance: Significance) -> Option<bool> {
        self.p_value.map(|p| p > significance.alpha())
    }

    fn test(name: &'static str, detail: String, p_value: f64) -> Self {
        Self {
            name,
            detail,
            p_value: Some(p_value),
            informational: false,
            skipped: false,
        }
    }

    fn skipped(name: &'static str, detail: String) -> Self {
        Self {
            name,
            detail,
            p_value: None,
            informational: false,
            skipped: true,
        }
    }

    fn informational(name: &'static str, detail: String) -> Self {
        Self {
            name,
            detail,
            p_value: None,
            informational: true,
            skipped: false,
        }
    }
}

/// Character-level result, or the reason it was skipped.
#[derive(Clone, Debug)]
pub enum CharacterAnalysis {
    Skipped { reason: String },
    Performed(CharacterResult),
}

impl CharacterAnalysis {
    pub fn skipped_reason(&self) -> Option<&str> {
        match self {
            Self::Skipped { reason } => Some(reason),
            Self::Performed(_) => None,
        }
    }
}

/// Summary of character count and transition tests.
#[derive(Clone, Debug)]
pub struct CharacterResult {
    pub alphabet: String,
    pub alphabet_size: usize,
    pub positions: usize,
    pub tested_positions: usize,
    pub insufficient_positions: usize,
    pub count_tests: usize,
    pub count_failures: usize,
    pub transition_tests: usize,
    pub transition_failures: usize,
    pub transition_note: Option<String>,
    pub small_sample: bool,
    pub total_bits: f64,
    pub entropy: Vec<EntropyPoint>,
}

/// Bit-position tests and whole-stream tests.
#[derive(Clone, Debug)]
pub struct BitAnalysis {
    pub positions: usize,
    pub tested_positions: usize,
    pub insufficient_positions: usize,
    pub balance_failures: usize,
    pub runs_failures: usize,
    pub correlation_failures: usize,
    pub total_bits: usize,
    pub entropy: Vec<EntropyPoint>,
    pub failing_positions: Vec<usize>,
    pub global_tests: Vec<TestResult>,
    pub global_bits: usize,
    pub global_truncated: bool,
}

/// A completed analysis, ready for the result window.
#[derive(Clone, Debug)]
pub struct Analysis {
    pub source: String,
    pub settings: AnalysisSettings,
    pub format: TokenFormat,
    pub sample_size: usize,
    pub shortest: usize,
    pub longest: usize,
    pub truncated: bool,
    pub character: CharacterAnalysis,
    pub bit: Option<BitAnalysis>,
    pub primary: AnalysisKind,
    pub entropy: Vec<EntropyPoint>,
    pub total_possible_bits: f64,
    pub primary_entropy: f64,
    pub rating: Rating,
    pub reliability: Reliability,
}

/// Analyze a token sample.
pub fn analyze(
    tokens: &[String],
    settings: AnalysisSettings,
    source: &str,
) -> Result<Analysis, String> {
    if tokens.len() < 2 {
        return Err(format!(
            "the Sequencer needs at least two tokens; {} loaded",
            tokens.len()
        ));
    }
    let shortest = tokens
        .iter()
        .map(|token| token_length(token))
        .min()
        .unwrap_or(0);
    let longest = tokens
        .iter()
        .map(|token| token_length(token))
        .max()
        .unwrap_or(0);
    let mut truncated = tokens.len() > MAX_ANALYSIS_TOKENS;

    let sample: Vec<String> = tokens.iter().take(MAX_ANALYSIS_TOKENS).cloned().collect();
    let format = if settings.format == TokenFormat::Auto {
        detect_format(&sample)
    } else {
        settings.format
    };
    let decoded = decode_tokens(&sample, format)?;
    let mut decoded: Vec<Vec<u8>> = decoded
        .into_iter()
        .map(|mut bytes| {
            if bytes.len() > MAX_ANALYSIS_BYTES {
                bytes.truncate(MAX_ANALYSIS_BYTES);
                truncated = true;
            }
            bytes
        })
        .collect();
    if decoded.iter().all(Vec::is_empty) {
        return Err("the sampled tokens decode to no bytes".to_string());
    }

    let char_tokens: Vec<Vec<char>> = sample
        .iter()
        .map(|token| {
            if token.chars().count() > MAX_ANALYSIS_CHARS {
                truncated = true;
            }
            token.chars().take(MAX_ANALYSIS_CHARS).collect()
        })
        .collect();

    let character = analyze_characters(&char_tokens, settings);
    let bit = if settings.bit_analysis {
        Some(analyze_bits(&decoded, settings.significance))
    } else {
        None
    };

    let character_result = match &character {
        CharacterAnalysis::Performed(result) if result.total_bits > 0.0 => Some(result),
        _ => None,
    };
    let (primary, entropy, total_possible_bits, primary_entropy) =
        if let Some(result) = character_result {
            (
                AnalysisKind::Character,
                result.entropy.clone(),
                result.total_bits,
                entropy_at(&result.entropy, settings.significance),
            )
        } else if let Some(bit) = &bit {
            (
                AnalysisKind::Bit,
                bit.entropy.clone(),
                bit.total_bits as f64,
                entropy_at(&bit.entropy, settings.significance),
            )
        } else {
            return Err(
                "no analysis could be performed; enable bit-level analysis or load more tokens"
                    .to_string(),
            );
        };

    let global_tests = bit
        .as_ref()
        .map(|bit| bit.global_tests.as_slice())
        .unwrap_or(&[]);
    let rating = rating(
        primary_entropy,
        total_possible_bits,
        global_tests,
        settings.significance,
    );
    let reliability = Reliability::from_tokens(tokens.len());

    // The bit-level decoder can accept tokens that are too short for any
    // character-level position. The UI still shows the bit-level tab, but the
    // headline uses whichever family produced a usable estimate.
    decoded.shrink_to_fit();

    Ok(Analysis {
        source: source.to_string(),
        settings,
        format,
        sample_size: tokens.len(),
        shortest,
        longest,
        truncated,
        character,
        bit,
        primary,
        entropy,
        total_possible_bits,
        primary_entropy,
        rating,
        reliability,
    })
}

fn analyze_characters(char_tokens: &[Vec<char>], settings: AnalysisSettings) -> CharacterAnalysis {
    if settings.character_analysis == CharacterAnalysisMode::Never {
        return CharacterAnalysis::Skipped {
            reason: "Character-level analysis was disabled in the analysis settings.".to_string(),
        };
    }

    let alphabet: Vec<char> = char_tokens
        .iter()
        .flatten()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let alphabet_size = alphabet.len();
    if alphabet_size < 2 {
        return CharacterAnalysis::Skipped {
            reason: "Character-level analysis was not performed because the sample contains fewer than two distinct characters.".to_string(),
        };
    }
    if alphabet_size > MAX_CHARACTER_ALPHABET {
        return CharacterAnalysis::Skipped {
            reason: format!(
                "Character-level analysis was not performed because the character set has {alphabet_size} symbols; the limit is {MAX_CHARACTER_ALPHABET}."
            ),
        };
    }

    let automatic = settings.character_analysis == CharacterAnalysisMode::Auto;
    let small_sample = char_tokens.len() < CHARACTER_SAMPLE_MULTIPLIER * alphabet_size;
    if automatic && small_sample {
        return CharacterAnalysis::Skipped {
            reason: "Note: Character-level analysis was not performed because the sample size is too small relative to the size of the character set used in the sampled tokens.".to_string(),
        };
    }

    let positions = char_tokens.iter().map(Vec::len).max().unwrap_or(0);
    let index: HashMap<char, usize> = alphabet
        .iter()
        .enumerate()
        .map(|(position, character)| (*character, position))
        .collect();
    let mut count_p = vec![None; positions];
    let mut count_tests = 0;
    let mut insufficient_positions = 0;

    for (position, slot) in count_p.iter_mut().enumerate() {
        let mut counts = vec![0usize; alphabet_size];
        let mut observations = 0;
        for token in char_tokens {
            if let Some(character) = token.get(position)
                && let Some(index) = index.get(character)
            {
                counts[*index] += 1;
                observations += 1;
            }
        }
        if observations < 2 {
            insufficient_positions += 1;
            continue;
        }
        let expected = observations as f64 / alphabet_size as f64;
        if expected < 1.0 {
            insufficient_positions += 1;
            continue;
        }
        let statistic: f64 = counts
            .iter()
            .map(|count| {
                let difference = *count as f64 - expected;
                difference * difference / expected
            })
            .sum();
        *slot = Some(chi_square_sf(statistic, alphabet_size - 1));
        count_tests += 1;
    }

    let mut transition_p = vec![None; positions.saturating_sub(1)];
    let mut transition_tests = 0;
    let transition_threshold = TRANSITION_SAMPLE_MULTIPLIER * alphabet_size * alphabet_size;
    for (position, slot) in transition_p.iter_mut().enumerate() {
        let mut matrix = vec![vec![0usize; alphabet_size]; alphabet_size];
        let mut observations = 0;
        for token in char_tokens {
            let (Some(first), Some(second)) = (token.get(position), token.get(position + 1)) else {
                continue;
            };
            let (Some(first), Some(second)) = (index.get(first), index.get(second)) else {
                continue;
            };
            matrix[*first][*second] += 1;
            observations += 1;
        }
        if observations < transition_threshold {
            continue;
        }
        let row_totals: Vec<usize> = matrix.iter().map(|row| row.iter().sum()).collect();
        let mut column_totals = vec![0usize; alphabet_size];
        for row in &matrix {
            for (column, count) in row.iter().enumerate() {
                column_totals[column] += count;
            }
        }
        let rows = row_totals.iter().filter(|total| **total > 0).count();
        let columns = column_totals.iter().filter(|total| **total > 0).count();
        if rows < 2 || columns < 2 {
            continue;
        }
        let mut statistic = 0.0;
        for (row_index, row) in matrix.iter().enumerate() {
            if row_totals[row_index] == 0 {
                continue;
            }
            for (column_index, count) in row.iter().enumerate() {
                if column_totals[column_index] == 0 {
                    continue;
                }
                let expected = row_totals[row_index] as f64 * column_totals[column_index] as f64
                    / observations as f64;
                if expected <= 0.0 {
                    continue;
                }
                let difference = *count as f64 - expected;
                statistic += difference * difference / expected;
            }
        }
        let degrees = (rows - 1) * (columns - 1);
        *slot = Some(chi_square_sf(statistic, degrees));
        transition_tests += 1;
    }

    let bits_per_position = (alphabet_size as f64).log2();
    let mut entropy = Vec::with_capacity(Significance::ALL.len());
    for significance in Significance::ALL {
        let alpha = significance.alpha();
        let mut bits = 0.0;
        for (position, count) in count_p.iter().enumerate() {
            let Some(count) = count else {
                continue;
            };
            if *count <= alpha {
                continue;
            }
            let previous = position
                .checked_sub(1)
                .and_then(|previous| transition_p.get(previous).copied().flatten());
            let next = transition_p.get(position).copied().flatten();
            if previous.is_some_and(|p| p <= alpha) || next.is_some_and(|p| p <= alpha) {
                continue;
            }
            bits += bits_per_position;
        }
        entropy.push(EntropyPoint { significance, bits });
    }

    let alpha = settings.significance.alpha();
    let count_failures = count_p
        .iter()
        .filter(|p| p.is_some_and(|p| p <= alpha))
        .count();
    let transition_failures = transition_p
        .iter()
        .filter(|p| p.is_some_and(|p| p <= alpha))
        .count();
    let tested_positions = count_p.iter().filter(|p| p.is_some()).count();
    let transition_note = if transition_tests == 0 && positions > 1 {
        Some(format!(
            "Transition tests need at least {transition_threshold} adjacent pairs for a {alphabet_size}-symbol alphabet."
        ))
    } else {
        None
    };

    CharacterAnalysis::Performed(CharacterResult {
        alphabet: display_alphabet(&alphabet),
        alphabet_size,
        positions,
        tested_positions,
        insufficient_positions,
        count_tests,
        count_failures,
        transition_tests,
        transition_failures,
        transition_note,
        small_sample,
        total_bits: tested_positions as f64 * bits_per_position,
        entropy,
    })
}

fn analyze_bits(decoded: &[Vec<u8>], significance: Significance) -> BitAnalysis {
    let positions = decoded
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .saturating_mul(8)
        .min(MAX_ANALYSIS_BITS);
    let mut tested_positions = 0;
    let mut insufficient_positions = 0;
    let mut balance_failures = 0;
    let mut runs_failures = 0;
    let mut correlation_failures = 0;
    let mut failing_positions = Vec::new();
    let mut pass_counts = [0usize; 5];

    for position in 0..positions {
        let byte_index = position / 8;
        let shift = 7 - (position % 8);
        let mut bits = Vec::with_capacity(decoded.len());
        for token in decoded {
            if let Some(byte) = token.get(byte_index) {
                bits.push((byte >> shift) & 1 == 1);
            }
        }
        if bits.len() < MIN_BIT_SAMPLES {
            insufficient_positions += 1;
            continue;
        }

        let ones = bits.iter().filter(|bit| **bit).count();
        let balance = binomial_two_sided(ones, bits.len(), 0.5);
        let runs = runs_test(&bits);
        let correlation = lag_one_autocorrelation(&bits);
        let mut combined = balance;
        let mut test_count = 1;
        if let Some(runs) = runs {
            combined = combined.min(runs);
            test_count += 1;
        }
        if let Some(correlation) = correlation {
            combined = combined.min(correlation);
            test_count += 1;
        }
        // Be conservative across the three tests on the same bit position.
        let combined = (combined * test_count as f64).min(1.0);

        tested_positions += 1;
        for (index, level) in Significance::ALL.iter().enumerate() {
            if combined > level.alpha() {
                pass_counts[index] += 1;
            }
        }
        if balance <= significance.alpha() {
            balance_failures += 1;
        }
        if runs.is_some_and(|p| p <= significance.alpha()) {
            runs_failures += 1;
        }
        if correlation.is_some_and(|p| p <= significance.alpha()) {
            correlation_failures += 1;
        }
        if combined <= significance.alpha() {
            failing_positions.push(position);
        }
    }

    let entropy = Significance::ALL
        .iter()
        .enumerate()
        .map(|(index, significance)| EntropyPoint {
            significance: *significance,
            bits: pass_counts[index] as f64,
        })
        .collect();
    let (global_tests, global_bits, global_truncated) = global_tests(decoded);

    BitAnalysis {
        positions,
        tested_positions,
        insufficient_positions,
        balance_failures,
        runs_failures,
        correlation_failures,
        total_bits: tested_positions,
        entropy,
        failing_positions,
        global_tests,
        global_bits,
        global_truncated,
    }
}

fn global_tests(decoded: &[Vec<u8>]) -> (Vec<TestResult>, usize, bool) {
    let mut bits = Vec::new();
    let mut truncated = false;
    'tokens: for token in decoded {
        for byte in token {
            for shift in (0..8).rev() {
                if bits.len() >= MAX_GLOBAL_BITS {
                    truncated = true;
                    break 'tokens;
                }
                bits.push((byte >> shift) & 1 == 1);
            }
        }
    }
    let mut tests = Vec::new();

    if bits.is_empty() {
        tests.push(TestResult::skipped(
            "Monobit frequency",
            "No bits were available for the test.".to_string(),
        ));
    } else {
        let ones = bits.iter().filter(|bit| **bit).count();
        let p = binomial_two_sided(ones, bits.len(), 0.5);
        tests.push(TestResult::test(
            "Monobit frequency",
            format!(
                "{} of {} bits are 1 ({:.2}%).",
                ones,
                bits.len(),
                ones as f64 * 100.0 / bits.len() as f64
            ),
            p,
        ));
    }

    if let Some(p) = runs_test(&bits) {
        let runs = 1 + bits.windows(2).filter(|pair| pair[0] != pair[1]).count();
        tests.push(TestResult::test(
            "Runs",
            format!("{} runs across {} bits.", runs, bits.len()),
            p,
        ));
    } else {
        tests.push(TestResult::skipped(
            "Runs",
            "At least two bits are needed.".to_string(),
        ));
    }

    let longest = longest_run_of_ones(&bits);
    if let Some(p) = longest_run_p_value(longest, bits.len()) {
        tests.push(TestResult::test(
            "Longest run of ones",
            format!("Longest run: {longest} bits."),
            p,
        ));
    } else {
        tests.push(TestResult::skipped(
            "Longest run of ones",
            "At least ten bits are needed.".to_string(),
        ));
    }

    if let Some((p, groups)) = poker_test(&bits) {
        tests.push(TestResult::test(
            "Poker (4-bit groups)",
            format!("{groups} non-overlapping groups."),
            p,
        ));
    } else {
        tests.push(TestResult::skipped(
            "Poker (4-bit groups)",
            "At least 80 four-bit groups (320 bits) are needed.".to_string(),
        ));
    }

    let bytes: Vec<u8> = decoded
        .iter()
        .flat_map(|token| token.iter().copied())
        .take(MAX_GLOBAL_BYTES)
        .collect();
    if let Some((p, count)) = byte_frequency_test(&bytes) {
        tests.push(TestResult::test(
            "Byte frequency",
            format!("{count} bytes across {} values.", 256),
            p,
        ));
    } else {
        tests.push(TestResult::skipped(
            "Byte frequency",
            "At least 1,280 bytes are needed for 256 byte values.".to_string(),
        ));
    }

    if let Some((_, adjusted, lag)) = pairwise_correlation_test(&bits) {
        tests.push(TestResult::test(
            "Pairwise correlation",
            format!("Strongest short-lag correlation at lag {lag}; Bonferroni-adjusted."),
            adjusted,
        ));
    } else {
        tests.push(TestResult::skipped(
            "Pairwise correlation",
            "At least 100 bits are needed.".to_string(),
        ));
    }

    if let Some(ratio) = compression_ratio(&bytes) {
        tests.push(TestResult::informational(
            "Zlib compression",
            format!("Compressed size is {:.1}% of the raw bytes.", ratio * 100.0),
        ));
    } else {
        tests.push(TestResult::skipped(
            "Zlib compression",
            "No bytes were available for compression.".to_string(),
        ));
    }

    (tests, bits.len(), truncated)
}

fn rating(
    primary_entropy: f64,
    total_possible_bits: f64,
    global_tests: &[TestResult],
    significance: Significance,
) -> Rating {
    let ratio = if total_possible_bits > 0.0 {
        (primary_entropy / total_possible_bits).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mut passed = 0;
    let mut applicable = 0;
    for test in global_tests {
        if test.informational || test.skipped {
            continue;
        }
        if let Some(pass) = test.passed_at(significance) {
            applicable += 1;
            if pass {
                passed += 1;
            }
        }
    }
    let global_ratio = if applicable == 0 {
        1.0
    } else {
        passed as f64 / applicable as f64
    };
    Rating::from_score(0.85 * ratio + 0.15 * global_ratio)
}

fn entropy_at(entropy: &[EntropyPoint], significance: Significance) -> f64 {
    entropy
        .iter()
        .find(|point| point.significance == significance)
        .map_or(0.0, |point| point.bits)
}

fn display_alphabet(alphabet: &[char]) -> String {
    let mut output = String::new();
    for character in alphabet.iter().take(48) {
        if character.is_control() || character.is_whitespace() {
            output.push_str(&character.escape_default().to_string());
        } else {
            output.push(*character);
        }
    }
    if alphabet.len() > 48 {
        output.push('…');
    }
    output
}

/// Format a bit count without a trailing `.0` for integral values.
pub fn format_bits(bits: f64) -> String {
    if (bits - bits.round()).abs() < 0.05 {
        format!("{:.0}", bits.round())
    } else {
        format!("{bits:.1}")
    }
}

/// Format a p-value compactly for the result tables.
pub fn format_p_value(p: f64) -> String {
    if p < 0.00001 {
        "<0.00001".to_string()
    } else if p < 0.001 {
        format!("{p:.5}")
    } else {
        format!("{p:.4}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_tokens(count: usize, length: usize) -> Vec<String> {
        let mut state = 0x1234_5678_9abc_def0u64;
        (0..count)
            .map(|_| {
                let mut token = String::with_capacity(length);
                for _ in 0..length {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    let nibble = (state >> 33) & 0xf;
                    token.push(char::from_digit(nibble as u32, 16).unwrap());
                }
                token
            })
            .collect()
    }

    fn dna_tokens(count: usize, length: usize) -> Vec<String> {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        (0..count)
            .map(|_| {
                let mut token = String::with_capacity(length);
                for _ in 0..length {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1);
                    let symbol = b"ACGT"[((state >> 33) & 3) as usize];
                    token.push(char::from(symbol));
                }
                token
            })
            .collect()
    }

    #[test]
    fn hex_sample_skips_character_tests_and_uses_bits() {
        let tokens = hex_tokens(100, 24);
        let analysis = analyze(&tokens, AnalysisSettings::default(), "manual load").unwrap();
        assert_eq!(analysis.format, TokenFormat::Hex);
        assert_eq!(analysis.sample_size, 100);
        assert_eq!(analysis.shortest, 24);
        assert_eq!(analysis.longest, 24);
        assert!(analysis.character.skipped_reason().is_some());
        assert_eq!(analysis.primary, AnalysisKind::Bit);
        assert_eq!(analysis.reliability, Reliability::VeryPoor);
        assert!(analysis.primary_entropy > 0.0);
        assert_eq!(analysis.total_possible_bits, 96.0);
        assert!(analysis.bit.as_ref().unwrap().global_tests.len() >= 5);
        assert!(matches!(
            analysis.rating,
            Rating::VeryGood | Rating::Good | Rating::Fair
        ));
    }

    #[test]
    fn constant_bit_positions_are_reported_as_failures() {
        let mut tokens = hex_tokens(100, 24);
        for token in &mut tokens {
            // Set the high bit of every token's first byte. The first bit
            // position is then constant and must fail the bit-level tests.
            let first = token.as_bytes()[0];
            let replacement = match first {
                b'0'..=b'7' => (first + 8) as char,
                _ => first as char,
            };
            token.replace_range(0..1, &replacement.to_string());
        }
        let analysis = analyze(&tokens, AnalysisSettings::default(), "manual load").unwrap();
        let bit = analysis.bit.unwrap();
        assert!(bit.failing_positions.contains(&0));
        assert!(bit.entropy[1].bits < bit.total_bits as f64);
    }

    #[test]
    fn large_text_sample_runs_character_level_tests() {
        let tokens = dna_tokens(500, 8);
        let settings = AnalysisSettings {
            format: TokenFormat::Text,
            ..AnalysisSettings::default()
        };
        let analysis = analyze(&tokens, settings, "live capture").unwrap();
        assert_eq!(analysis.format, TokenFormat::Text);
        let character = match &analysis.character {
            CharacterAnalysis::Performed(result) => result,
            CharacterAnalysis::Skipped { reason } => {
                panic!("character tests were skipped: {reason}")
            }
        };
        assert_eq!(character.alphabet_size, 4);
        assert_eq!(character.positions, 8);
        assert_eq!(character.count_tests, 8);
        assert!(character.transition_tests > 0);
        assert_eq!(analysis.primary, AnalysisKind::Character);
        assert!(character.total_bits > 0.0);
    }

    #[test]
    fn forced_character_analysis_runs_on_a_small_sample() {
        let tokens = vec!["AB".to_string(), "CD".to_string(), "AC".to_string()];
        let settings = AnalysisSettings {
            character_analysis: CharacterAnalysisMode::Always,
            ..AnalysisSettings::default()
        };
        let analysis = analyze(&tokens, settings, "manual load").unwrap();
        let character = match &analysis.character {
            CharacterAnalysis::Performed(result) => result,
            CharacterAnalysis::Skipped { reason } => {
                panic!("character tests were skipped: {reason}")
            }
        };
        assert!(character.small_sample);
        assert_eq!(character.alphabet_size, 4);
    }

    #[test]
    fn disabled_bit_analysis_with_skipped_character_analysis_errors() {
        let tokens = hex_tokens(100, 24);
        let settings = AnalysisSettings {
            bit_analysis: false,
            ..AnalysisSettings::default()
        };
        let error = analyze(&tokens, settings, "manual load").unwrap_err();
        assert!(error.contains("no analysis could be performed"));
    }

    #[test]
    fn invalid_hex_reports_a_token_number() {
        let tokens = vec!["abcd".to_string(), "nope".to_string()];
        let settings = AnalysisSettings {
            format: TokenFormat::Hex,
            ..AnalysisSettings::default()
        };
        let error = analyze(&tokens, settings, "manual load").unwrap_err();
        assert!(error.contains("token 2"));
    }

    #[test]
    fn reliability_thresholds_follow_sample_size() {
        assert_eq!(Reliability::from_tokens(199), Reliability::VeryPoor);
        assert_eq!(Reliability::from_tokens(200), Reliability::Poor);
        assert_eq!(Reliability::from_tokens(499), Reliability::Poor);
        assert_eq!(Reliability::from_tokens(500), Reliability::Fair);
        assert_eq!(Reliability::from_tokens(999), Reliability::Fair);
        assert_eq!(Reliability::from_tokens(1_000), Reliability::Good);
        assert_eq!(Reliability::from_tokens(4_999), Reliability::Good);
        assert_eq!(Reliability::from_tokens(5_000), Reliability::Excellent);
    }

    #[test]
    fn significance_labels_match_the_chart() {
        assert_eq!(Significance::TenPercent.chart_label(), ">10%");
        assert_eq!(Significance::OnePercent.percent_label(), "1%");
        assert_eq!(Significance::PointZeroZeroOnePercent.alpha(), 0.00001);
    }

    #[test]
    fn bit_formatting_is_compact() {
        assert_eq!(format_bits(8.0), "8");
        assert_eq!(format_bits(7.96), "8");
        assert_eq!(format_bits(8.4), "8.4");
        assert_eq!(format_p_value(0.000001), "<0.00001");
        assert_eq!(format_p_value(0.5), "0.5000");
    }

    #[test]
    fn analysis_truncates_large_samples_and_long_tokens() {
        let tokens = hex_tokens(MAX_ANALYSIS_TOKENS + 1, 24);
        let analysis = analyze(&tokens, AnalysisSettings::default(), "manual load").unwrap();
        assert!(analysis.truncated);
        assert_eq!(analysis.sample_size, MAX_ANALYSIS_TOKENS + 1);
    }

    #[test]
    fn labels_cover_every_setting_and_result_variant() {
        assert_eq!(Significance::TenPercent.percent_label(), "10%");
        assert_eq!(Significance::PointOnePercent.percent_label(), "0.1%");
        assert_eq!(Significance::PointZeroOnePercent.percent_label(), "0.01%");
        assert_eq!(
            Significance::PointZeroZeroOnePercent.percent_label(),
            "0.001%"
        );
        assert_eq!(Significance::OnePercent.chart_label(), ">1%");
        assert_eq!(Significance::PointOnePercent.chart_label(), ">0.1%");
        assert_eq!(Significance::PointZeroOnePercent.chart_label(), ">0.01%");
        assert_eq!(
            Significance::PointZeroZeroOnePercent.chart_label(),
            ">0.001%"
        );
        assert_eq!(Significance::TenPercent.alpha(), 0.10);
        assert_eq!(Significance::OnePercent.alpha(), 0.01);
        assert_eq!(Significance::PointOnePercent.alpha(), 0.001);
        assert_eq!(Significance::PointZeroOnePercent.alpha(), 0.0001);

        assert_eq!(Rating::VeryPoor.label(), "very poor");
        assert_eq!(Rating::Poor.label(), "poor");
        assert_eq!(Rating::Fair.label(), "fair");
        assert_eq!(Rating::Good.label(), "good");
        assert_eq!(Rating::VeryGood.label(), "very good");

        assert_eq!(Reliability::VeryPoor.label(), "very poor");
        assert_eq!(Reliability::Poor.label(), "poor");
        assert_eq!(Reliability::Fair.label(), "fair");
        assert_eq!(Reliability::Good.label(), "good");
        assert_eq!(Reliability::Excellent.label(), "excellent");

        assert_eq!(AnalysisKind::Character.label(), "character-level");
        assert_eq!(AnalysisKind::Bit.label(), "bit-level");
        assert_eq!(CharacterAnalysisMode::Always.label(), "Always");
        assert_eq!(CharacterAnalysisMode::Never.label(), "Never");
    }

    #[test]
    fn analyze_rejects_short_samples_and_disabled_character_tests() {
        let error = analyze(
            &["one".to_string()],
            AnalysisSettings::default(),
            "manual load",
        )
        .unwrap_err();
        assert!(error.contains("at least two tokens"));

        let tokens = dna_tokens(400, 8);
        let settings = AnalysisSettings {
            character_analysis: CharacterAnalysisMode::Never,
            ..AnalysisSettings::default()
        };
        let analysis = analyze(&tokens, settings, "manual load").unwrap();
        assert!(
            analysis
                .character
                .skipped_reason()
                .is_some_and(|reason| reason.contains("disabled"))
        );
        assert_eq!(analysis.primary, AnalysisKind::Bit);
    }

    #[test]
    fn character_tests_skip_tiny_and_oversized_alphabets_and_truncate_long_tokens() {
        let single = analyze(
            &["AAAA".to_string(), "AAAA".to_string()],
            AnalysisSettings {
                format: TokenFormat::Text,
                ..AnalysisSettings::default()
            },
            "manual load",
        )
        .unwrap();
        assert!(
            single
                .character
                .skipped_reason()
                .is_some_and(|reason| reason.contains("fewer than two distinct"))
        );

        let alphabet: String = (0..=MAX_CHARACTER_ALPHABET)
            .map(|code| char::from_u32(0x100 + code as u32).unwrap())
            .collect();
        let oversized = analyze(
            &[alphabet.clone(), alphabet],
            AnalysisSettings {
                format: TokenFormat::Text,
                ..AnalysisSettings::default()
            },
            "manual load",
        )
        .unwrap();
        assert!(
            oversized
                .character
                .skipped_reason()
                .is_some_and(|reason| reason.contains("character set has"))
        );

        let long = "ab".repeat(MAX_ANALYSIS_CHARS);
        let truncated = analyze(
            &[long.clone(), long],
            AnalysisSettings {
                format: TokenFormat::Text,
                character_analysis: CharacterAnalysisMode::Always,
                ..AnalysisSettings::default()
            },
            "manual load",
        )
        .unwrap();
        assert!(truncated.truncated);
        assert_eq!(truncated.longest, MAX_ANALYSIS_CHARS * 2);
        let character = match truncated.character {
            CharacterAnalysis::Performed(result) => result,
            CharacterAnalysis::Skipped { reason } => panic!("unexpected skip: {reason}"),
        };
        assert_eq!(character.positions, MAX_ANALYSIS_CHARS);
    }

    #[test]
    fn character_positions_with_too_few_observations_are_counted_as_insufficient() {
        let mut tokens: Vec<Vec<char>> = (0..40)
            .map(|index| {
                if index % 2 == 0 {
                    "ABC".chars().collect()
                } else {
                    "AB".chars().collect()
                }
            })
            .collect();
        tokens.push(vec!['A', 'B', 'C', 'D']);
        let result = match analyze_characters(
            &tokens,
            AnalysisSettings {
                character_analysis: CharacterAnalysisMode::Always,
                ..AnalysisSettings::default()
            },
        ) {
            CharacterAnalysis::Performed(result) => result,
            CharacterAnalysis::Skipped { reason } => panic!("unexpected skip: {reason}"),
        };
        assert_eq!(result.positions, 4);
        assert_eq!(result.insufficient_positions, 1);
    }

    #[test]
    fn transition_tests_skip_one_sided_matrices_and_failing_positions() {
        let mut one_row: Vec<Vec<char>> = (0..40)
            .map(|index| vec!['A', if index % 2 == 0 { 'A' } else { 'B' }])
            .collect();
        one_row.extend((0..40).map(|index| vec!['A', if index % 2 == 0 { 'B' } else { 'A' }]));
        let result = match analyze_characters(
            &one_row,
            AnalysisSettings {
                character_analysis: CharacterAnalysisMode::Always,
                ..AnalysisSettings::default()
            },
        ) {
            CharacterAnalysis::Performed(result) => result,
            CharacterAnalysis::Skipped { reason } => panic!("unexpected skip: {reason}"),
        };
        assert_eq!(result.alphabet_size, 2);
        assert_eq!(result.transition_tests, 0);
        assert!(result.transition_note.is_some());

        let mut alternating: Vec<Vec<char>> = (0..80)
            .map(|index| {
                (0..20)
                    .map(|position| {
                        if (index + position) % 2 == 0 {
                            'A'
                        } else {
                            'B'
                        }
                    })
                    .collect()
            })
            .collect();
        alternating.push(vec!['A', 'B']);
        let result = match analyze_characters(
            &alternating,
            AnalysisSettings {
                character_analysis: CharacterAnalysisMode::Always,
                significance: Significance::TenPercent,
                ..AnalysisSettings::default()
            },
        ) {
            CharacterAnalysis::Performed(result) => result,
            CharacterAnalysis::Skipped { reason } => panic!("unexpected skip: {reason}"),
        };
        assert!(result.transition_tests > 0);
    }

    #[test]
    fn global_tests_report_skipped_checks_for_empty_input() {
        let (tests, bits, truncated) = global_tests(&[]);
        assert_eq!(bits, 0);
        assert!(!truncated);
        assert!(
            tests
                .iter()
                .any(|test| test.name == "Monobit frequency" && test.skipped)
        );
        assert!(tests.iter().any(|test| test.name == "Runs" && test.skipped));
        assert!(
            tests
                .iter()
                .any(|test| test.name == "Longest run of ones" && test.skipped)
        );
        assert!(
            tests
                .iter()
                .any(|test| test.name == "Zlib compression" && test.skipped)
        );
    }

    #[test]
    fn global_tests_truncate_after_the_bit_cap() {
        let decoded = vec![vec![0xAA; MAX_GLOBAL_BITS / 8 + 1]];
        let (tests, bits, truncated) = global_tests(&decoded);
        assert!(truncated);
        assert_eq!(bits, MAX_GLOBAL_BITS);
        assert!(tests.iter().any(|test| test.name == "Monobit frequency"));
    }

    #[test]
    fn bit_analysis_counts_positions_without_enough_samples() {
        let bit = analyze_bits(&[vec![0xAA, 0xBB], vec![0xCC]], Significance::OnePercent);
        assert_eq!(bit.positions, 16);
        assert_eq!(bit.insufficient_positions, 8);
        assert_eq!(bit.tested_positions, 8);
    }

    #[test]
    fn rating_handles_zero_totals_and_no_applicable_tests() {
        assert_eq!(
            rating(0.0, 0.0, &[], Significance::OnePercent),
            Rating::VeryPoor
        );
        assert_eq!(
            rating(10.0, 10.0, &[], Significance::OnePercent),
            Rating::VeryGood
        );
        assert_eq!(
            rating(0.0, 10.0, &[], Significance::OnePercent),
            Rating::VeryPoor
        );
    }

    #[test]
    fn alphabet_display_escapes_controls_and_truncates() {
        let alphabet: Vec<char> = ['\n', ' ', 'a', 'b']
            .into_iter()
            .chain((0..50).map(|code| char::from_u32(0x200 + code).unwrap()))
            .collect();
        let display = display_alphabet(&alphabet);
        assert!(display.contains("\\n"));
        assert!(display.ends_with('…'));
    }

    #[test]
    fn p_values_use_the_intermediate_precision() {
        assert_eq!(format_p_value(0.0005), "0.00050");
    }
}
