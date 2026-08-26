//! Content-type classifier for BeliefNodes.
//!
//! Scores each node's text to produce an N/S/P/R content profile using two
//! complementary signals:
//!
//! - **N (Normative)** — signal words: modal verbs (shall/must/should/may),
//!   constraint language (within/exceed/tolerance), negation (not/no/never)
//! - **P (Procedural)** — signal words: imperative verbs (begin/execute/verify),
//!   sequential markers (then/next/step), logic conjunctions (if/else/while)
//! - **S (Structural)** — weak lexically; primarily a graph-level signal
//!   (Issue 85 force layout handles this via edge topology)
//! - **R (Record)** — token-shape heuristics: numeric density, unit patterns,
//!   timestamp patterns, key-value structure
//!
//! Scores are independent (not a simplex) — a node can score high on multiple
//! axes simultaneously.

use std::collections::HashSet;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Signal word sets (stemmed forms)
// ---------------------------------------------------------------------------

/// Normative signal words — modal verbs, constraint language, negation.
///
/// These are the stemmed forms that survive `tokenize()`. They capture the
/// grammatical patterns of normative text (passive voice + modals + constraints)
/// without domain-specific nouns.
fn normative_signals() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&str>> = OnceLock::new();
    SET.get_or_init(|| {
        [
            // Modal verbs (RFC 2119 keywords)
            "shall",
            "must",
            "should",
            "may",
            "might",
            "can",
            "could",
            // Negation / scope restriction
            "not",
            "no",
            "never",
            "without",
            "only",
            // Constraint language (stemmed forms)
            "within",
            "exceed",
            "limit",
            "minimum",
            "maximum",
            "toler",
            "accept",
            "rang",
            "constrain",
            "threshold",
            // Requirements language (stemmed forms)
            "requir",
            "specifi",
            "compli",
            "mandat",
            "ensur",
        ]
        .into_iter()
        .collect()
    })
}

/// Procedural signal words — dynamic language: imperative verbs, causal
/// verbs, state transitions, sequential markers, logic conjunctions.
///
/// P content is fundamentally *dynamic* — it has temporal extent and
/// causation. These capture that dynamism without domain-specific nouns.
fn procedural_signals() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&str>> = OnceLock::new();
    SET.get_or_init(|| {
        [
            // Imperative verbs (stemmed forms)
            "begin", "execut", "verifi", "perform", "connect", "remov", "appli", "configur",
            "inspect", "follow", "proceed", "initi", "activ", "run", "drain", "upload", "assembl",
            "conduct", "captur", "repeat", "confirm", "enabl", "disabl", "insert", "launch",
            "shut", "start",
            // Causal verbs (stemmed forms)
            // "gener" removed: stems from "general"/"generation" (not P)
            //   as often as "generate" (P).
            // "propagat"/"induc" removed: more physics-descriptive than procedural.
            "caus", "trigger", "produc", "yield", "invok", "emit",
            // State transition verbs (stemmed forms)
            "transit", "switch", "enter", "exit", "chang", "becom", "toggl", "reset", "halt",
            // Future tense / sequential markers
            "will", "then", "next", "step",
            // "first" removed: primarily an ordinal adjective ("first-order"),
            // not a sequential marker in engineering text.
            // Logic conjunctions
            "if", "else", "unless", "while", "until", // Action completeness
            "complet",
        ]
        .into_iter()
        .collect()
    })
}

// ---------------------------------------------------------------------------
// ContentProfile
// ---------------------------------------------------------------------------

/// Per-node content-type profile: four independent scores on \[0, 1\].
///
/// N, S, P are the three spatial dimensions of model-space (normative,
/// structural, procedural). R detects temporal anchoring — content bound
/// to a specific observation event. High R flags content susceptible to
/// expiration.
///
/// Scores are NOT a normalized simplex — a node can score high on all
/// axes simultaneously.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ContentProfile {
    pub n: f32,
    pub s: f32,
    pub p: f32,
    pub r: f32,
}

impl ContentProfile {
    /// Serialize to a TOML table with keys `n`, `s`, `p`, `r`.
    pub fn to_toml(&self) -> toml::Table {
        let mut table = toml::Table::new();
        table.insert("n".to_string(), toml::Value::Float(self.n as f64));
        table.insert("s".to_string(), toml::Value::Float(self.s as f64));
        table.insert("p".to_string(), toml::Value::Float(self.p as f64));
        table.insert("r".to_string(), toml::Value::Float(self.r as f64));
        table
    }

    /// Returns `true` if all four scores are exactly zero.
    pub fn is_zero(&self) -> bool {
        self.n == 0.0 && self.s == 0.0 && self.p == 0.0 && self.r == 0.0
    }
}

// ---------------------------------------------------------------------------
// EdgeCounts
// ---------------------------------------------------------------------------

/// Edge counts by WeightKind and direction, used as input to structural scoring.
#[derive(Debug, Clone, Default)]
pub struct EdgeCounts {
    pub section_in: u32,
    pub section_out: u32,
    pub epistemic_in: u32,
    pub epistemic_out: u32,
    pub pragmatic_in: u32,
    pub pragmatic_out: u32,
    pub owned_edge_count: u32,
}

// ---------------------------------------------------------------------------
// Scoring functions
// ---------------------------------------------------------------------------

/// Score a node's text using signal-word density, voice/tense detection,
/// and token-shape heuristics.
///
/// - **N**: signal-word density + passive voice patterns in raw text
/// - **P**: signal-word density (imperative verbs, logic conjunctions)
/// - **S**: 0.0 (structural classification is a graph-level concern)
/// - **R**: past-tense density + numeric/timestamp/unit heuristics
///
/// `tokens` are the stemmed output of `tokenize()`. `raw_text` is the original
/// unstemmed text (needed for voice/tense and shape heuristics).
///
/// Returns a zero profile if both token list and raw text are empty.
pub fn score_lexical(tokens: &[String], raw_text: &str) -> ContentProfile {
    let n_score = score_normative(tokens, raw_text);
    let p_score = score_procedural(tokens);
    let r_score = score_record(raw_text);

    ContentProfile {
        n: n_score,
        s: 0.0,
        p: p_score,
        r: r_score,
    }
}

/// Normative score: signal-word density + passive voice density.
///
/// Passive voice (be-form + past participle) is a strong N signal:
/// both requirements ("shall be maintained") and descriptive/definitional
/// text ("is configured with") specify entity boundaries.
fn score_normative(tokens: &[String], raw_text: &str) -> f32 {
    // Signal-word component (from stemmed tokens)
    let signal_score = if tokens.is_empty() {
        0.0
    } else {
        let signals = normative_signals();
        let hits = tokens
            .iter()
            .filter(|t| signals.contains(t.as_str()))
            .count();
        hits as f32 / tokens.len() as f32
    };

    // Passive voice component (from raw text)
    let passive_score = passive_voice_density(raw_text);

    // Blend: signal words are the primary indicator, passive voice
    // is supporting evidence.
    (0.6 * signal_score + 0.4 * passive_score).min(1.0)
}

/// Fraction of tokens that are procedural signal words.
fn score_procedural(tokens: &[String]) -> f32 {
    if tokens.is_empty() {
        return 0.0;
    }
    let signals = procedural_signals();
    let hits = tokens
        .iter()
        .filter(|t| signals.contains(t.as_str()))
        .count();
    (hits as f32 / tokens.len() as f32).min(1.0)
}

// ---------------------------------------------------------------------------
// Voice / tense detection
// ---------------------------------------------------------------------------

/// Be-form words that precede a past participle in passive constructions.
fn be_forms() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&str>> = OnceLock::new();
    SET.get_or_init(|| {
        ["is", "are", "was", "were", "be", "been", "being"]
            .into_iter()
            .collect()
    })
}

/// Returns `true` if a word looks like a past participle (ends in "-ed",
/// at least 4 characters to avoid "ed", "red", etc.).
fn is_past_participle(word: &str) -> bool {
    word.len() >= 4 && word.ends_with("ed")
}

/// Fraction of word-pairs that are passive voice constructions
/// (be-form + past participle).
///
/// Scans raw text for bigrams like "is maintained", "are specified",
/// "shall be verified". Returns a density score on [0, 1].
fn passive_voice_density(raw_text: &str) -> f32 {
    let words: Vec<&str> = raw_text
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .collect();
    if words.len() < 2 {
        return 0.0;
    }
    let be = be_forms();
    let passive_count = words
        .windows(2)
        .filter(|pair| {
            let prev = pair[0].to_lowercase();
            let curr = pair[1].to_lowercase();
            be.contains(prev.as_str()) && is_past_participle(&curr)
        })
        .count();
    // Normalize: one passive construction per ~15 words is moderate density.
    let density = passive_count as f32 / (words.len() as f32 / 15.0).max(1.0);
    density.min(1.0)
}

/// Fraction of words in raw text that end in "-ed" (past tense / past
/// participle), as an R signal.
///
/// Observations are reported in past tense: "measured", "observed",
/// "recorded", "passed", "failed". High past-tense density suggests
/// content anchored to a specific moment.
fn past_tense_density(raw_text: &str) -> f32 {
    let words: Vec<&str> = raw_text
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| w.len() >= 4)
        .collect();
    if words.is_empty() {
        return 0.0;
    }
    let past_count = words
        .iter()
        .filter(|w| is_past_participle(&w.to_lowercase()))
        .count();
    (past_count as f32 / words.len() as f32).min(1.0)
}

// ---------------------------------------------------------------------------
// R scoring
// ---------------------------------------------------------------------------

/// Score R (record/observation) from raw text.
///
/// Combines tense detection (past-tense density) with token-shape
/// heuristics (timestamps, commit hashes, numeric density, units).
/// Past tense is the strongest signal: R content reports what already
/// happened.
fn score_record(raw_text: &str) -> f32 {
    if raw_text.is_empty() {
        return 0.0;
    }

    let chars: Vec<char> = raw_text.chars().collect();
    let total_chars = chars.len() as f32;

    // 1. Numeric density: fraction of characters that are digits or decimal points
    let digit_chars = chars
        .iter()
        .filter(|c| c.is_ascii_digit() || **c == '.')
        .count() as f32;
    let numeric_density = digit_chars / total_chars;

    // 2. Colon-pattern density: count of colons normalized by text length.
    //    Key-value structure ("channel: 3", "status: pass") is typical of
    //    data records. Normalize per 100 characters to avoid over-scoring
    //    short text with incidental colons.
    let colon_count = raw_text.matches(':').count() as f32;
    let colon_density = (colon_count / (total_chars / 100.0).max(1.0)).min(1.0);

    // 3. Unit-like tokens: check each whitespace-delimited word against
    //    known measurement unit stems. Word-boundary matching avoids false
    //    positives like "meter" inside "parameters".
    static UNIT_SET: OnceLock<HashSet<&str>> = OnceLock::new();
    let units = UNIT_SET.get_or_init(|| {
        [
            "volt",
            "volts",
            "watt",
            "watts",
            "ampere",
            "amperes",
            "milliamp",
            "milliamps",
            "hertz",
            "ohm",
            "ohms",
            "degree",
            "degrees",
            "percent",
            "kilogram",
            "kilograms",
            "gram",
            "grams",
            "meter",
            "meters",
            "millimeter",
            "millimeters",
            "microsecond",
            "microseconds",
            "millisecond",
            "milliseconds",
            "celsius",
            "fahrenheit",
            "pascal",
            "pascals",
            "newton",
            "newtons",
            "joule",
            "joules",
            "decibel",
            "decibels",
            // Abbreviated forms (only match as standalone words)
            "mhz",
            "khz",
            "ghz",
            "mv",
            "ma",
            "kw",
            "mw",
            "mm",
            "cm",
            "km",
            "kg",
            "ms",
            "ns",
            "db",
            "psi",
            "rpm",
        ]
        .into_iter()
        .collect()
    });
    let unit_hits = raw_text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| {
            let lower = word.to_lowercase();
            units.contains(lower.as_str())
        })
        .count() as f32;
    let unit_score = (unit_hits / 3.0).min(1.0);

    // 4. Timestamp patterns: HH:MM:SS, YYYY-MM-DD, ISO 8601 variants.
    //    These pin content to a specific moment — the defining property of R.
    let timestamp_hits = count_timestamp_patterns(raw_text) as f32;
    let timestamp_score = (timestamp_hits / 2.0).min(1.0);

    // 5. Commit hashes: 7+ contiguous hex characters that aren't plain words.
    //    These pin content to a specific model-state (a coordinate in
    //    model-spacetime, same as timestamps).
    let hash_hits = count_hash_patterns(raw_text) as f32;
    let hash_score = (hash_hits / 2.0).min(1.0);

    // 6. Past-tense density: fraction of words ending in "-ed".
    //    Observations are reported in past tense — the strongest
    //    grammatical signal that content is anchored to a specific moment.
    let past_tense = past_tense_density(raw_text);

    // Weighted combination, clamped to [0, 1].
    // Past tense and timestamps are the strongest R signals (temporal
    // anchoring). Hashes, numeric density, and units are supporting evidence.
    let score = 0.30 * past_tense
        + 0.25 * timestamp_score
        + 0.15 * hash_score
        + 0.15 * numeric_density
        + 0.05 * colon_density
        + 0.10 * unit_score;
    score.min(1.0)
}

/// Count timestamp-like patterns in raw text.
///
/// Matches:
/// - Time: `HH:MM`, `HH:MM:SS`, `HH:MM:SS.fff`
/// - Date: `YYYY-MM-DD`, `YYYY/MM/DD`, `DD/MM/YYYY`, `MM/DD/YYYY`
/// - ISO 8601: `YYYY-MM-DDTHH:MM:SS`
///
/// Returns the number of distinct matches found.
fn count_timestamp_patterns(text: &str) -> usize {
    let mut count = 0;
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Look for digit sequences that could start a timestamp
        if bytes[i].is_ascii_digit() {
            // Try YYYY-MM-DD or YYYY/MM/DD (4 digit year)
            if i + 9 < len && (bytes[i + 4] == b'-' || bytes[i + 4] == b'/') {
                let sep = bytes[i + 4];
                if (sep == b'-' || sep == b'/')
                    && bytes[i + 1].is_ascii_digit()
                    && bytes[i + 2].is_ascii_digit()
                    && bytes[i + 3].is_ascii_digit()
                    && bytes[i + 5].is_ascii_digit()
                    && bytes[i + 6].is_ascii_digit()
                    && bytes[i + 7] == sep
                    && bytes[i + 8].is_ascii_digit()
                    && bytes[i + 9].is_ascii_digit()
                {
                    count += 1;
                    i += 10;
                    continue;
                }
            }
            // Try HH:MM or HH:MM:SS
            if i + 4 < len
                && bytes[i + 1].is_ascii_digit()
                && bytes[i + 2] == b':'
                && bytes[i + 3].is_ascii_digit()
                && bytes[i + 4].is_ascii_digit()
            {
                count += 1;
                i += 5;
                // Skip optional :SS
                if i + 2 < len
                    && bytes[i] == b':'
                    && bytes[i + 1].is_ascii_digit()
                    && bytes[i + 2].is_ascii_digit()
                {
                    i += 3;
                }
                continue;
            }
        }
        i += 1;
    }
    count
}

/// Count commit-hash-like patterns: 7+ contiguous lowercase hex characters
/// that are not plain English words.
///
/// Heuristic: a run of 7-40 characters from `[0-9a-f]` that contains at
/// least one digit (to exclude words like "abcdef" or "facade").
fn count_hash_patterns(text: &str) -> usize {
    let mut count = 0;
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        let len = word.len();
        if (7..=40).contains(&len)
            && word
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            && word.chars().any(|c| c.is_ascii_digit())
        {
            count += 1;
        }
    }
    count
}

/// Score a node's structural position from its edge topology.
///
/// Maps edge counts by WeightKind and direction to an N/S/P bias vector.
/// R is always 0 — edges encode spatial relationships, not temporal anchoring.
///
/// Mapping:
/// - High outgoing epistemic (depends on) → N-like
/// - High incoming epistemic (depended upon) → S-like
/// - High outgoing pragmatic (consumes) → P-like
/// - High incoming pragmatic (consumed by) → S-like
/// - Owned edges (maps\_to directives) → P-like (traceability action)
///
/// Returns a zero profile if all counts are zero.
pub fn score_structural(edges: &EdgeCounts) -> ContentProfile {
    let raw_n = edges.epistemic_out as f32;
    let raw_s = (edges.epistemic_in + edges.pragmatic_in) as f32;
    let raw_p = (edges.pragmatic_out + edges.owned_edge_count) as f32;

    let max = raw_n.max(raw_s).max(raw_p);
    if max <= 0.0 {
        return ContentProfile::default();
    }

    ContentProfile {
        n: raw_n / max,
        s: raw_s / max,
        p: raw_p / max,
        r: 0.0,
    }
}

/// Default lexical weight for blending.
const ALPHA_DEFAULT: f32 = 0.7;

/// Default per-axis divergence threshold for warning emission.
const DIVERGENCE_THRESHOLD_DEFAULT: f32 = 0.4;

/// Read the divergence threshold from the environment, falling back to
/// [`DIVERGENCE_THRESHOLD_DEFAULT`] if unset or unparseable.
fn divergence_threshold() -> f32 {
    std::env::var("NOET_CONTENT_DIVERGENCE_THRESHOLD")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(DIVERGENCE_THRESHOLD_DEFAULT)
}

/// Result of [`score_merge`]: the blended profile plus any channel
/// divergence warnings.
#[derive(Debug, Clone)]
pub struct MergeResult {
    /// The α-blended content profile.
    pub profile: ContentProfile,
    /// Warning messages for axes where lexical and structural scores
    /// diverge by more than the configured threshold.
    pub warnings: Vec<String>,
}

/// Blend lexical and structural profiles.
///
/// `merged = α × lexical + (1 - α) × structural`
///
/// Signal-availability fallback:
/// - `lexical.is_zero()` → α = 0 (structure only)
/// - `structural.is_zero()` → α = 1 (lexical only)
/// - Both non-zero → α = default (0.7)
///
/// When both channels are non-zero, per-axis divergence is checked:
/// if `|lexical.x - structural.x| > threshold` for any axis x ∈ {N, S, P},
/// a warning string is emitted. The threshold defaults to 0.4 and is
/// tunable via the `NOET_CONTENT_DIVERGENCE_THRESHOLD` environment variable.
pub fn score_merge(lexical: &ContentProfile, structural: &ContentProfile) -> MergeResult {
    let alpha = if lexical.is_zero() {
        0.0
    } else if structural.is_zero() {
        1.0
    } else {
        ALPHA_DEFAULT
    };

    let beta = 1.0 - alpha;

    let profile = ContentProfile {
        n: alpha * lexical.n + beta * structural.n,
        s: alpha * lexical.s + beta * structural.s,
        p: alpha * lexical.p + beta * structural.p,
        r: alpha * lexical.r + beta * structural.r,
    };

    let mut warnings = Vec::new();

    // Only check divergence when both channels contributed signal.
    if !lexical.is_zero() && !structural.is_zero() {
        let threshold = divergence_threshold();
        let axes: [(&str, f32, f32); 3] = [
            ("N (normative)", lexical.n, structural.n),
            ("S (structural)", lexical.s, structural.s),
            ("P (procedural)", lexical.p, structural.p),
        ];
        for (label, lex, struc) in axes {
            let delta = (lex - struc).abs();
            if delta > threshold {
                let direction = if struc > lex {
                    "high structural, low lexical"
                } else {
                    "high lexical, low structural"
                };
                warnings.push(format!(
                    "content-type divergence on {label}: {direction} \
                     (lexical={lex:.2}, structural={struc:.2}, delta={delta:.2})"
                ));
            }
        }
    }

    MergeResult { profile, warnings }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shard::search::Stemmer;

    fn stemmer() -> Stemmer {
        Stemmer::new()
    }

    fn make_tokens(text: &str) -> Vec<String> {
        crate::shard::search::tokenize(text, &stemmer()).collect()
    }

    // -- score_lexical: normative detection --

    #[test]
    fn lexical_normative_text_scores_highest_on_n() {
        let tokens = make_tokens(
            "The system shall maintain a maximum response latency. \
             All interfaces must comply with the specification. \
             The component shall not exceed the allocated power budget.",
        );
        let profile = score_lexical(&tokens, "");
        assert!(
            profile.n > profile.p,
            "N ({}) should exceed P ({})",
            profile.n,
            profile.p
        );
        assert!(profile.n > 0.1, "N ({}) should be substantial", profile.n);
    }

    #[test]
    fn lexical_procedural_text_scores_highest_on_p() {
        let tokens = make_tokens(
            "Begin by powering on the equipment. Execute the test sequence. \
             Verify that all interlocks are engaged before proceeding. \
             If the reading exceeds the limit, then repeat the calibration step.",
        );
        let profile = score_lexical(&tokens, "");
        assert!(
            profile.p > profile.n,
            "P ({}) should exceed N ({})",
            profile.p,
            profile.n
        );
        assert!(profile.p > 0.1, "P ({}) should be substantial", profile.p);
    }

    #[test]
    fn lexical_record_text_scores_on_r() {
        let raw = "2024-03-15 14:32:07 output voltage 5.03 volts measured, \
                   tolerance 5.0 plus or minus 0.1 volts, result: pass. \
                   Sensor channel 3: transient spike 12.4 milliamps, \
                   duration 45 milliseconds.";
        let profile = score_lexical(&[], raw);
        assert!(
            profile.r > 0.05,
            "R ({}) should be non-trivial for data-dense text",
            profile.r
        );
    }

    #[test]
    fn lexical_structural_text_scores_low_on_s() {
        let tokens = make_tokens(
            "The architecture consists of three layers. \
             Component A interfaces with Component B through a message queue.",
        );
        let profile = score_lexical(&tokens, "");
        assert!(
            profile.s < 0.01,
            "S ({}) should be near-zero lexically",
            profile.s
        );
    }

    #[test]
    fn lexical_empty_returns_zero() {
        let profile = score_lexical(&[], "");
        assert!(profile.is_zero(), "empty input should produce zero profile");
    }

    // -- voice / tense detection --

    #[test]
    fn passive_voice_boosts_normative() {
        let raw = "The interface is configured with three endpoints. \
                   The system is designed to operate in two modes. \
                   Parameters are validated before processing.";
        let tokens = make_tokens(raw);
        let profile = score_lexical(&tokens, raw);
        assert!(
            profile.n > 0.05,
            "passive voice should contribute to N ({})",
            profile.n
        );
    }

    #[test]
    fn past_tense_boosts_record() {
        let raw = "Voltage was measured at 5.03 volts. \
                   The crack was observed during inspection. \
                   All checks passed and the unit was accepted.";
        let score = score_record(raw);
        assert!(score > 0.05, "past-tense text should score R ({})", score);
    }

    #[test]
    fn passive_voice_density_basics() {
        assert!(passive_voice_density("is maintained") > 0.0);
        assert!(passive_voice_density("are specified") > 0.0);
        assert!(passive_voice_density("were observed") > 0.0);
        assert_eq!(passive_voice_density("run the test"), 0.0);
        assert_eq!(passive_voice_density(""), 0.0);
    }

    #[test]
    fn past_tense_density_basics() {
        let high = past_tense_density("measured observed recorded passed failed");
        let low = past_tense_density("measure observe record pass fail");
        assert!(
            high > low,
            "past tense ({}) should exceed present ({})",
            high,
            low
        );
        assert_eq!(past_tense_density(""), 0.0);
    }

    // -- score_record heuristics --

    #[test]
    fn record_high_numeric_density() {
        let raw = "14:32:07 5.03 12.4 45 0.1 2.3 0.02 95 312 500 14.7";
        let score = score_record(raw);
        assert!(
            score > 0.15,
            "numeric-dense text should score high R ({})",
            score
        );
    }

    #[test]
    fn record_prose_scores_low() {
        let raw = "The system architecture consists of three primary layers \
                   with a shared memory bus connecting all processing modules.";
        let score = score_record(raw);
        assert!(score < 0.05, "prose text should score low R ({})", score);
    }

    #[test]
    fn record_unit_patterns_contribute() {
        let raw = "measured 5.03 volts, 12.4 milliamps, 127 hertz";
        let score = score_record(raw);
        assert!(score > 0.1, "text with units should score R ({})", score);
    }

    #[test]
    fn record_timestamps_score_high() {
        let raw = "2024-03-15 14:32:07 voltage 5.03, status pass";
        let score = score_record(raw);
        assert!(
            score > 0.15,
            "text with timestamps should score high R ({})",
            score
        );
    }

    #[test]
    fn record_commit_hashes_score_high() {
        let raw = "commit a1b2c3d merged into main, parent 9f8e7d6";
        let score = score_record(raw);
        assert!(
            score > 0.1,
            "text with commit hashes should score R ({})",
            score
        );
    }

    #[test]
    fn record_timestamp_counter_basics() {
        assert_eq!(count_timestamp_patterns("14:32:07"), 1);
        assert_eq!(count_timestamp_patterns("2024-03-15"), 1);
        assert_eq!(count_timestamp_patterns("2024-03-15 14:32:07"), 2);
        assert_eq!(count_timestamp_patterns("no timestamps here"), 0);
    }

    #[test]
    fn record_hash_counter_basics() {
        assert_eq!(count_hash_patterns("a1b2c3d"), 1, "7-char hex with digit");
        assert_eq!(
            count_hash_patterns("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0"),
            1,
            "40-char full SHA"
        );
        assert_eq!(count_hash_patterns("abcdef"), 0, "6 chars too short");
        assert_eq!(count_hash_patterns("ABCDEF1"), 0, "uppercase rejected");
        assert_eq!(
            count_hash_patterns("abcdefg"),
            0,
            "no digits → likely a word"
        );
    }

    // -- score_structural tests --

    #[test]
    fn structural_high_epistemic_out_scores_highest_n() {
        let edges = EdgeCounts {
            epistemic_out: 10,
            ..Default::default()
        };
        let profile = score_structural(&edges);
        assert!(
            profile.n > profile.s && profile.n > profile.p,
            "epistemic_out should dominate N: {:?}",
            profile
        );
    }

    #[test]
    fn structural_high_epistemic_in_scores_highest_s() {
        let edges = EdgeCounts {
            epistemic_in: 10,
            ..Default::default()
        };
        let profile = score_structural(&edges);
        assert!(
            profile.s > profile.n && profile.s > profile.p,
            "epistemic_in should dominate S: {:?}",
            profile
        );
    }

    #[test]
    fn structural_high_pragmatic_out_scores_highest_p() {
        let edges = EdgeCounts {
            pragmatic_out: 10,
            ..Default::default()
        };
        let profile = score_structural(&edges);
        assert!(
            profile.p > profile.n && profile.p > profile.s,
            "pragmatic_out should dominate P: {:?}",
            profile
        );
    }

    #[test]
    fn structural_owned_edges_contribute_to_p() {
        let edges = EdgeCounts {
            owned_edge_count: 10,
            ..Default::default()
        };
        let profile = score_structural(&edges);
        assert!(
            profile.p > profile.n && profile.p > profile.s,
            "owned_edge_count should contribute to P: {:?}",
            profile
        );
    }

    #[test]
    fn structural_zero_counts_returns_zero() {
        let edges = EdgeCounts::default();
        let profile = score_structural(&edges);
        assert!(profile.is_zero(), "zero edges → zero profile");
    }

    #[test]
    fn structural_r_always_zero() {
        let edges = EdgeCounts {
            epistemic_out: 5,
            pragmatic_in: 3,
            owned_edge_count: 7,
            ..Default::default()
        };
        let profile = score_structural(&edges);
        assert_eq!(profile.r, 0.0, "R should always be 0 in structural scoring");
    }

    // -- score_merge tests --

    #[test]
    fn merge_both_nonzero_blends_at_alpha_0_7() {
        let lexical = ContentProfile {
            n: 1.0,
            s: 0.0,
            p: 0.0,
            r: 0.0,
        };
        let structural = ContentProfile {
            n: 0.0,
            s: 1.0,
            p: 0.0,
            r: 0.0,
        };
        let result = score_merge(&lexical, &structural);
        let merged = result.profile;
        let eps = 1e-6;
        assert!(
            (merged.n - 0.7).abs() < eps,
            "n should be 0.7, got {}",
            merged.n
        );
        assert!(
            (merged.s - 0.3).abs() < eps,
            "s should be 0.3, got {}",
            merged.s
        );
    }

    #[test]
    fn merge_zero_lexical_returns_structural() {
        let lexical = ContentProfile::default();
        let structural = ContentProfile {
            n: 0.5,
            s: 0.8,
            p: 0.3,
            r: 0.0,
        };
        let result = score_merge(&lexical, &structural);
        assert_eq!(
            result.profile, structural,
            "zero lexical → should return structural"
        );
        assert!(
            result.warnings.is_empty(),
            "no divergence warnings when one channel is zero"
        );
    }

    #[test]
    fn merge_zero_structural_returns_lexical() {
        let lexical = ContentProfile {
            n: 0.6,
            s: 0.0,
            p: 0.9,
            r: 0.1,
        };
        let structural = ContentProfile::default();
        let result = score_merge(&lexical, &structural);
        assert_eq!(
            result.profile, lexical,
            "zero structural → should return lexical"
        );
        assert!(
            result.warnings.is_empty(),
            "no divergence warnings when one channel is zero"
        );
    }

    #[test]
    fn merge_divergence_emits_warnings() {
        let lexical = ContentProfile {
            n: 0.1,
            s: 0.0,
            p: 0.5,
            r: 0.0,
        };
        let structural = ContentProfile {
            n: 0.9,
            s: 0.5,
            p: 0.5,
            r: 0.0,
        };
        let result = score_merge(&lexical, &structural);
        assert!(
            result.warnings.iter().any(|w| w.contains("N (normative)")),
            "should warn about N divergence: {:?}",
            result.warnings
        );
    }

    #[test]
    fn merge_no_divergence_when_similar() {
        let lexical = ContentProfile {
            n: 0.5,
            s: 0.0,
            p: 0.5,
            r: 0.0,
        };
        let structural = ContentProfile {
            n: 0.6,
            s: 0.3,
            p: 0.3,
            r: 0.0,
        };
        let result = score_merge(&lexical, &structural);
        assert!(
            result.warnings.is_empty(),
            "no warnings when delta <= threshold: {:?}",
            result.warnings
        );
    }

    // -- ContentProfile tests --

    #[test]
    fn to_toml_round_trips() {
        let profile = ContentProfile {
            n: 0.82,
            s: 0.15,
            p: 0.71,
            r: 0.05,
        };
        let table = profile.to_toml();

        let get = |key: &str| table.get(key).unwrap().as_float().unwrap() as f32;
        let eps = 1e-6;
        assert!((get("n") - 0.82).abs() < eps);
        assert!((get("s") - 0.15).abs() < eps);
        assert!((get("p") - 0.71).abs() < eps);
        assert!((get("r") - 0.05).abs() < eps);
    }

    #[test]
    fn is_zero_on_default() {
        assert!(ContentProfile::default().is_zero());
    }

    #[test]
    fn is_zero_false_when_any_nonzero() {
        let p = ContentProfile {
            n: 0.0,
            s: 0.0,
            p: 0.01,
            r: 0.0,
        };
        assert!(!p.is_zero());
    }
}
