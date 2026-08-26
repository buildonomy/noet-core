//! Compile-time search index building.
//!
//! Generates per-network `search/{bref}.idx.msgpack` files during `finalize_html`.
//! These files are always produced — regardless of whether data sharding is
//! active — so the viewer can search the entire corpus from the moment it loads,
//! even before any data shard is fetched.
//!
//! ## Index Format
//!
//! Each `.idx.msgpack` file is a compact inverted index for one network,
//! serialized as msgpack (same structure as the JSON below, binary-encoded):
//!
//! ```msgpack
//! {
//!   "network_bref": "01abc",
//!   "doc_count": 247,
//!   "docs": {
//!     "<bid>": { "title": "Installation Guide", "path": "docs/install.html", "term_count": 342 }
//!   },
//!   "index": {
//!     "instal": [["<bid>", 12], ["<bid2>", 3]],
//!     "guid":   [["<bid>", 8]]
//!   }
//! }
//! ```
//!
//! **`docs`**: Minimal per-document metadata for displaying search result rows
//! (title, path) and computing TF-IDF length normalization (term_count).
//!
//! **`index`**: `term → [(bid, frequency)]`. Title terms are indexed with a 3×
//! weight multiplier baked into the frequency count. Terms are lowercased,
//! split on whitespace and punctuation, filtered for English stop words, and
//! English-stemmed (Snowball algorithm) when the `stemming` feature is enabled
//! (default for `bin` builds).
//!
//! ## Stop Words
//!
//! Common English function words ("the", "a", "is", "and", etc.) are removed
//! during tokenization. Stop words add noise and bulk without improving search
//! quality — a query for "the installation guide" should match on "instal" and
//! "guid", not on "the". The stop word list is applied before stemming so the
//! stemmer never processes them. Query terms must apply the same filter.
//!
//! ## Stemming
//!
//! When the `stemming` feature is active, [`tokenize`] applies the Snowball
//! English stemmer from `rust-stemmers` as a final step. This means index terms
//! are stems, not raw words: "running" → "run", "installation" → "instal".
//! The WASM query side (Issue 54) must apply the **same** stemming to query
//! terms before index lookup. Both sides use the same Snowball English algorithm
//! — the compile-time side via `rust-stemmers`, the WASM side via the equivalent
//! JS implementation (e.g. `lunr` stemmer or a WASM-compiled Snowball port).
//!
//! When the `stemming` feature is absent, raw lowercased tokens are stored.
//! The query side detects the index version field (reserved, always `"1.0"` for
//! now) to know which mode was used — or simply always stems, which is harmless
//! if the index was already stemmed.
//!
//! ## References
//!
//! - `docs/design/search_and_sharding.md` §7.2 — Index format
//! - `docs/design/search_and_sharding.md` §7.3 — Index building algorithm
//! - Issue 50: BeliefBase Sharding (generates the files)
//! - Issue 54: Full-Text Search MVP (deserializes and queries the `.idx.msgpack` files)

#[cfg(not(target_arch = "wasm32"))]
use crate::codec::is_network_index_file;
#[cfg(not(target_arch = "wasm32"))]
use crate::properties::{BeliefNode, Bid, Bref};
#[cfg(not(target_arch = "wasm32"))]
use crate::{
    error::BuildonomyError,
    paths::PathMapMap,
    shard::manifest::{NetworkSearchMeta, SearchManifest},
};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

/// Warn when a single network's search index exceeds this size (bytes).
///
/// A large index is a proxy for a large network. Authors should consider
/// splitting the network or removing low-value content.
/// 5MB index → roughly 100–150MB of source text.
#[cfg(not(target_arch = "wasm32"))]
const LARGE_INDEX_WARN_BYTES: usize = 5 * 1024 * 1024;

/// Standard English stop words filtered out during tokenization.
///
/// Applied before stemming so the stemmer never processes these tokens.
/// The query side (Issue 54 WASM) must apply the same filter to query terms
/// so that stop words in a query don't produce zero results.
///
/// List derived from the standard Snowball English stop word list, trimmed
/// to the ~100 most frequent function words. Contractions are already
/// handled by the apostrophe-stripping step in `tokenize`.
static ENGLISH_STOP_WORDS: std::sync::OnceLock<BTreeSet<&'static str>> = std::sync::OnceLock::new();

fn stop_words() -> &'static BTreeSet<&'static str> {
    ENGLISH_STOP_WORDS.get_or_init(|| {
        [
            // Articles
            "a",
            "an",
            "the",
            // Conjunctions
            "and",
            "but",
            "or",
            "nor",
            "for",
            "yet",
            "so",
            "both",
            "either",
            "neither",
            // "not" — preserved: negation inverts meaning in engineering text
            // "only" — preserved: scope restriction ("only when...")
            "whether",
            "although",
            "because",
            "since",
            // "unless" — preserved: conditional logic in requirements
            "until",
            "while",
            "though",
            "even",
            // Prepositions
            "at",
            "by",
            "in",
            "of",
            "on",
            "to",
            "up",
            "as",
            "into",
            "from",
            "with",
            "about",
            "above",
            "after",
            "against",
            "along",
            "among",
            "around",
            "before",
            "behind",
            "below",
            "beneath",
            "beside",
            "between",
            "beyond",
            "during",
            "except",
            "inside",
            "near",
            "off",
            "out",
            "outside",
            "over",
            "past",
            "per",
            "through",
            "throughout",
            "under",
            "underneath",
            "upon",
            "via",
            // "within" — preserved: constraint language ("within tolerance")
            // "without" — preserved: negation-like ("without loss of functionality")
            // Pronouns
            "i",
            "me",
            "my",
            "we",
            "us",
            "our",
            "you",
            "your",
            "he",
            "him",
            "his",
            "she",
            "her",
            "hers",
            "it",
            "its",
            "they",
            "them",
            "their",
            "who",
            "whom",
            "which",
            "what",
            "that",
            "this",
            "these",
            "those",
            "myself",
            "yourself",
            "himself",
            "herself",
            "itself",
            "ourselves",
            "themselves",
            // Common verbs (forms of be/have/do/will)
            "be",
            "is",
            "am",
            "are",
            "was",
            "were",
            "been",
            "being",
            "have",
            "has",
            "had",
            "having",
            "do",
            "does",
            "did",
            "doing",
            // "will" — preserved: future tense, procedural signal
            "would",
            // Modal verbs — preserved: RFC 2119 keywords with specific
            // engineering meaning. "shall" vs "should" vs "may" distinguishes
            // mandatory from advisory from permissive requirements.
            // "shall",
            // "should",
            // "may",
            // "might",
            // "must",
            // "can",
            // "could",
            "get",
            "got",
            "let",
            // Common adverbs / discourse markers
            // "no" — preserved: negation ("no single failure shall...")
            "yes",
            // "not" — preserved: negation inverts meaning
            "also",
            "just",
            "then",
            "than",
            "now",
            "here",
            "there",
            "when",
            "where",
            "why",
            "how",
            // "all" — preserved: universal quantifier ("all interfaces must...")
            // "any" — preserved: existential quantifier ("any condition")
            // "each" — preserved: distributive quantifier ("each subsystem shall...")
            "more",
            "most",
            "other",
            "some",
            "such",
            "same",
            "own",
            "few",
            "very",
            "too",
            "so",
            "well",
            "back",
            "still",
            "already",
            "again",
            "once",
            // "always" — preserved: temporal quantifier with constraint meaning
            // "never" — preserved: temporal quantifier with constraint meaning
            "ever",
            "often",
            "however",
            "therefore",
            "thus",
            "hence",
            "else",
            // "if" — preserved: conditional logic in requirements
        ]
        .iter()
        .copied()
        .collect()
    })
}

/// Whether stemming was applied during index construction.
///
/// Stored in [`SearchIndex::stemmed`] so the WASM query side can apply
/// the same transformation to query terms. When `false`, query terms must
/// be matched as-is (case-insensitive lowercase).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StemMode {
    /// Snowball English stemmer was applied. Query terms must be stemmed too.
    English,
    /// No stemming. Query terms are matched as lowercase verbatim tokens.
    None,
}

/// Multiplier applied to title term frequencies relative to body text.
///
/// Title terms are indexed as if they appeared 3× more often than body terms.
/// This biases TF-IDF scores toward documents whose title matches the query,
/// which is almost always the most relevant result for a given term.
#[cfg(not(target_arch = "wasm32"))]
const TITLE_WEIGHT: u32 = 3;

/// Node ID terms are indexed with higher weight than titles so that searching
/// for a known ID (e.g. `REQ-3080`, `TICKET-822`) ranks the exact node first.
#[cfg(not(target_arch = "wasm32"))]
const ID_WEIGHT: u32 = 5;

/// Weight for the raw (unstemmed, lowercased) node ID indexed as a single
/// compound term. This ensures exact ID matches like `class-a` or `req-3080`
/// rank above partial token matches.
#[cfg(not(target_arch = "wasm32"))]
const ID_EXACT_WEIGHT: u32 = 10;

/// The active stem mode for this build.
///
/// When the `stemming` feature is enabled this is [`StemMode::English`];
/// otherwise [`StemMode::None`]. Used to populate [`SearchIndex::stemmed`].
#[cfg(all(not(target_arch = "wasm32"), feature = "stemming"))]
const ACTIVE_STEM_MODE: StemMode = StemMode::English;
#[cfg(all(not(target_arch = "wasm32"), not(feature = "stemming")))]
const ACTIVE_STEM_MODE: StemMode = StemMode::None;

/// Minimal per-document record stored in the search index.
///
/// Contains only what is needed to render a search result row and to compute
/// TF-IDF scores. Full node data (payload, relations) remains in the data
/// shard and is not duplicated here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexedDoc {
    /// Document title (always available, even for unloaded shards).
    pub title: String,
    /// HTML path to the document, relative to the viewer root.
    /// Empty string if no path is available (e.g. network root node).
    pub path: String,
    /// Total number of indexed terms (title + body) for TF-IDF normalization.
    pub term_count: u32,
    /// Schema name (e.g. `"requirement"`, `"hazard"`), if present.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schema: String,
    /// Kind labels (e.g. `"Document"`, `"Network"`), comma-separated.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
}

/// Compile-time inverted index for a single network.
///
/// Serialized to `search/{bref}.idx.msgpack` during `finalize_html`. The WASM
/// side (Issue 54) deserializes this and runs TF-IDF queries against it —
/// no index construction happens in the browser.
///
/// See `docs/design/search_and_sharding.md` §7.2 for the index schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchIndex {
    /// Short reference (5 hex chars) of the network this index covers.
    pub network_bref: String,
    /// Total number of indexed documents.
    pub doc_count: usize,
    /// Whether English Snowball stemming was applied to index terms.
    ///
    /// The WASM query side must apply the same stemming to query terms before
    /// lookup. `StemMode::None` means terms are stored as lowercase verbatim.
    pub stemmed: StemMode,
    /// Per-document metadata keyed by BID string.
    ///
    /// Maps `bid_string → IndexedDoc`. The BID string is the UUID form used
    /// everywhere else in the codebase (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`).
    pub docs: BTreeMap<String, IndexedDoc>,
    /// Inverted index: `field:term → [(bid_string, frequency)]`.
    ///
    /// Keys are field-prefixed: `"title:thruster"`, `"text:thruster"`,
    /// `"id:req"`, `"schema:requirement"`, `"kind:document"`. The special
    /// `"*:term"` prefix is used as a catch-all for queries without a field
    /// prefix.
    ///
    /// Frequencies for title terms are pre-multiplied by `TITLE_WEIGHT`.
    /// Entries within each posting list are sorted descending by frequency for
    /// fast top-K retrieval.
    pub index: BTreeMap<String, Vec<(String, u32)>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl SearchIndex {
    fn new(network_bref: Bref) -> Self {
        Self {
            network_bref: network_bref.to_string(),
            doc_count: 0,
            stemmed: ACTIVE_STEM_MODE,
            docs: BTreeMap::new(),
            index: BTreeMap::new(),
        }
    }

    /// Index a single document node.
    ///
    /// - `bid`: document BID
    /// - `node`: the `BeliefNode` to index (title + `payload["text"]`)
    /// - `path`: HTML-relative path for the search result row
    /// - `stemmer`: shared stemmer instance (constructed once per `build_search_indices` call)
    fn index_node(&mut self, bid: Bid, node: &BeliefNode, path: &str, stemmer: &Stemmer) {
        // Skip nodes with no meaningful content to index.
        let title = node.title.trim().to_string();
        let body_text = node
            .payload
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Use the explicit ID only — bref-derived fallback IDs are not
        // meaningful search terms.
        let node_id = match &node.id {
            crate::properties::NodeId::Explicit(s) if !s.is_empty() => s.clone(),
            _ => String::new(),
        };

        // Extract schema and kind for field-scoped search.
        let schema = node.schema.as_deref().unwrap_or("").to_string();
        let kind = node
            .kind
            .iter()
            .map(|k| format!("{k:?}"))
            .collect::<Vec<_>>()
            .join(",")
            .to_lowercase();

        if title.is_empty() && body_text.is_empty() && node_id.is_empty() {
            return;
        }

        let bid_str = bid.to_string();

        // Accumulate field:term → frequency for this document.
        // Each term is stored under both a field-specific key (e.g. "title:foo")
        // and a catch-all key ("*:foo") so that unscoped queries match all fields.
        let mut term_freqs: BTreeMap<String, u32> = BTreeMap::new();

        // Helper: add a term with both field-scoped and catch-all keys.
        let mut add_term = |field: &str, term: &str, weight: u32| {
            *term_freqs.entry(format!("{field}:{term}")).or_insert(0) += weight;
            *term_freqs.entry(format!("*:{term}")).or_insert(0) += weight;
        };

        // Index the node's ID with the highest boost.
        if !node_id.is_empty() {
            // Index tokenized fragments (e.g. "class-a" → "class").
            for term in tokenize(&node_id, stemmer) {
                add_term("id", &term, ID_WEIGHT);
            }
            // Index the raw ID as a single compound term so exact matches
            // (e.g. query "class-a" matching id "class-a") rank first.
            let raw_id = node_id.to_lowercase();
            add_term("id", &raw_id, ID_EXACT_WEIGHT);
        }

        // Index title terms with boosted weight.
        for term in tokenize(&title, stemmer) {
            add_term("title", &term, TITLE_WEIGHT);
        }

        // Index body terms with unit weight.
        for term in tokenize(&body_text, stemmer) {
            add_term("text", &term, 1);
        }

        // Index schema as a single term (not tokenized — schemas are identifiers).
        if !schema.is_empty() {
            let schema_lower = schema.to_lowercase();
            add_term("schema", &schema_lower, TITLE_WEIGHT);
        }

        // Index kind labels.
        if !kind.is_empty() {
            for k in kind.split(',') {
                let k = k.trim();
                if !k.is_empty() {
                    add_term("kind", k, TITLE_WEIGHT);
                }
            }
        }

        if term_freqs.is_empty() {
            return;
        }

        let term_count: u32 = term_freqs.values().sum();

        // Record per-document metadata.
        self.docs.insert(
            bid_str.clone(),
            IndexedDoc {
                title,
                path: path.to_string(),
                term_count,
                schema,
                kind,
            },
        );
        self.doc_count += 1;

        // Update the inverted index.
        for (term, freq) in term_freqs {
            self.index
                .entry(term)
                .or_default()
                .push((bid_str.clone(), freq));
        }
    }

    /// Sort each posting list descending by frequency.
    ///
    /// Called once after all documents have been indexed. Sorted lists let the
    /// WASM query side quickly take the top-K results without a full sort.
    fn finalize(&mut self) {
        for postings in self.index.values_mut() {
            postings.sort_unstable_by_key(|b| std::cmp::Reverse(b.1));
        }
    }
}

/// A thin wrapper that provides a uniform `.stem(word)` interface regardless of
/// whether the `stemming` feature is enabled.
///
/// Constructed once per `build_search_indices` call and shared across all
/// networks, avoiding repeated allocations.
pub struct Stemmer {
    #[cfg(feature = "stemming")]
    inner: rust_stemmers::Stemmer,
}

impl std::fmt::Debug for Stemmer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stemmer").finish()
    }
}

impl Stemmer {
    /// Create a new stemmer instance.
    ///
    /// With `stemming` feature: uses the Snowball English algorithm.
    /// Without: a zero-cost no-op shim.
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "stemming")]
            inner: rust_stemmers::Stemmer::create(rust_stemmers::Algorithm::English),
        }
    }

    /// Stem a single lowercase token, returning the stemmed form.
    ///
    /// Input must already be lowercase. Returns a `String` in both feature
    /// variants so call sites are identical.
    #[inline]
    pub fn stem(&self, word: &str) -> String {
        #[cfg(feature = "stemming")]
        {
            self.inner.stem(word).into_owned()
        }
        #[cfg(not(feature = "stemming"))]
        {
            word.to_string()
        }
    }
}

impl Default for Stemmer {
    fn default() -> Self {
        Self::new()
    }
}

/// Tokenize text into stemmed lowercase terms.
///
/// This tokenizer runs at compile time only. The WASM search side (Issue 54)
/// must apply the same tokenization and stemming logic so that query terms
/// match index terms.
///
/// Rules applied in order:
/// 1. Split on any character that is not alphanumeric or `'` (apostrophe)
/// 2. Lowercase
/// 3. Strip leading/trailing apostrophes
/// 4. Discard tokens shorter than 2 characters
/// 5. Discard purely numeric tokens (version numbers, years add noise)
/// 6. Discard English stop words (see `stop_words()`)
/// 7. Apply English Snowball stemming (when `stemming` feature is enabled)
///
/// The `stemmer` argument is passed in (constructed once per index build) so
/// this function avoids repeated allocations across millions of tokens.
pub fn tokenize<'a>(text: &'a str, stemmer: &'a Stemmer) -> impl Iterator<Item = String> + 'a {
    let stops = stop_words();
    text.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter_map(move |tok| {
            let lower = tok.to_lowercase();
            let lower = lower.trim_matches('\''); // strip leading/trailing apostrophes
            if lower.len() < 2 {
                return None;
            }
            // Discard purely numeric tokens
            if lower.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            // Discard stop words before stemming — no point stemming "the".
            if stops.contains(lower) {
                return None;
            }
            Some(stemmer.stem(lower))
        })
}

/// A single search result from a TF-IDF query against a [`SearchIndex`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// BID of the matching document.
    pub bid: String,
    /// Bref of the home network.
    pub network_bref: String,
    /// Document title (always available from the index).
    pub title: String,
    /// HTML-relative path (may be empty for network root nodes).
    pub path: String,
    /// TF-IDF relevance score. Higher is more relevant.
    pub score: f64,
}

/// Maximum query-term length for which fuzzy matching is attempted.
///
/// Levenshtein on long terms is both expensive and imprecise. Terms at or
/// above this length are matched exactly only.
const FUZZY_MAX_QUERY_TERM_LEN: usize = 20;

/// Score multipliers for fuzzy (non-exact) term matches by edit distance.
///
/// Index 0 is unused. Index 1 = distance-1 penalty, index 2 = distance-2 penalty.
/// Mirrors the `FUZZY_PENALTY` constants from the removed `search.js` engine.
const FUZZY_PENALTY: [f64; 3] = [1.0, 0.6, 0.3];

/// Compute the Levenshtein edit distance between two strings, with an early-exit
/// bound. Returns `None` if the distance exceeds `max_dist`.
///
/// Uses the standard Wagner-Fischer DP algorithm with a two-row rolling buffer
/// and a per-row minimum tracking early exit.
fn levenshtein(a: &str, b: &str, max_dist: usize) -> Option<usize> {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();

    // Fast path: length difference alone exceeds the bound.
    if m.abs_diff(n) > max_dist {
        return None;
    }

    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];

    for i in 1..=m {
        curr[0] = i;
        let mut row_min = curr[0];
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
            row_min = row_min.min(curr[j]);
        }
        // If the minimum value in this row already exceeds the bound, no
        // subsequent rows can produce a smaller distance.
        if row_min > max_dist {
            return None;
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    let dist = prev[n];
    if dist <= max_dist {
        Some(dist)
    } else {
        None
    }
}

/// Expand a set of exact query terms with fuzzy neighbours from an index.
///
/// For each query term shorter than [`FUZZY_MAX_QUERY_TERM_LEN`], walks the
/// index term list and collects all terms within Levenshtein distance ≤ 2 that
/// are not already exact matches. Returns a list of `(index_term, penalty)`
/// pairs representing additional terms to score, where `penalty` is taken from
/// [`FUZZY_PENALTY`].
///
/// This is called once per index (not per network) to amortise the cost of
/// materialising the term list.
fn fuzzy_expand<'a>(query_terms: &[String], index_terms: &'a [&'a str]) -> Vec<(&'a str, f64)> {
    let mut extras: Vec<(&'a str, f64)> = Vec::new();
    for query_term in query_terms {
        if query_term.len() >= FUZZY_MAX_QUERY_TERM_LEN {
            continue;
        }
        for &idx_term in index_terms {
            // Index keys are field-prefixed (e.g. "*:softwar", "title:softwar").
            // Extract the bare term after the colon for Levenshtein comparison.
            let bare_idx_term = match idx_term.find(':') {
                Some(pos) => &idx_term[pos + 1..],
                None => idx_term,
            };
            // Skip if this is already an exact match — handled at full weight.
            if bare_idx_term == query_term.as_str() {
                continue;
            }
            if let Some(dist) = levenshtein(query_term, bare_idx_term, 2) {
                if dist > 0 {
                    extras.push((idx_term, FUZZY_PENALTY[dist]));
                }
            }
        }
    }
    extras
}

/// Run a TF-IDF query against one or more pre-built [`SearchIndex`] instances.
///
/// This is the query-time counterpart to the compile-time [`build_search_indices`]
/// builder. It runs on both native (MCP server) and wasm32 (browser viewer) targets.
///
/// ## Algorithm
///
/// 1. Tokenize the query using the same rules as index building (split, lowercase,
///    stop-word filter, Snowball English stemming when `stemming` feature is active).
/// 2. For each `(index_term, postings)` that matches a query term exactly, compute:
///    - `idf = log((total_doc_count + 1) / (df + 1)) + 1`  (smoothed Laplace IDF)
///    - `tf  = raw_freq / term_count`  (length-normalised term frequency)
///    - score contribution = `tf × idf`
/// 3. For query terms shorter than `FUZZY_MAX_QUERY_TERM_LEN`, also score index
///    terms within Levenshtein distance ≤ 2, penalised by `FUZZY_PENALTY`.
/// 4. Accumulate per-document scores across all provided indices.
/// 5. Sort descending by score and return the top `limit` results.
///
/// ## Arguments
///
/// * `indices`  — Slice of search indices to query. All indices contribute to the
///   global `total_doc_count` used in IDF calculation, matching the
///   behaviour of the JS `runQuery` function in `search.js`.
/// * `query`    — Raw query string. Tokenized internally.
/// * `limit`    — Maximum number of results to return (0 = no limit).
///
/// ## Returns
///
/// A `Vec<SearchResult>` sorted descending by TF-IDF score, capped at `limit`.
/// A parsed query term with optional field scope and boolean mode.
#[derive(Debug, Clone)]
struct QueryTerm {
    /// Field prefix (e.g. `"title"`, `"schema"`). Empty string = catch-all (`*`).
    field: String,
    /// Stemmed term to look up in the index.
    term: String,
    /// Boolean mode: `And` means required, `Not` means excluded, `Or` is default.
    mode: BoolMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoolMode {
    Or,
    And,
    Not,
}

/// Parse query string into structured terms with field scope and boolean mode.
///
/// Syntax:
/// - `word` → catch-all search (`*:word`)
/// - `field:word` → field-scoped search (`field:word`)
/// - `AND` token → next term is required
/// - `NOT` token → next term is excluded
///
/// Unrecognized field prefixes are accepted (they just won't match anything).
fn parse_query_terms(query: &str, stemmer: &Stemmer) -> Vec<QueryTerm> {
    let mut result = Vec::new();
    let mut next_mode = BoolMode::Or;

    // Split on whitespace, then process each token.
    for raw_token in query.split_whitespace() {
        let upper = raw_token.to_uppercase();
        if upper == "AND" {
            next_mode = BoolMode::And;
            continue;
        }
        if upper == "NOT" {
            next_mode = BoolMode::Not;
            continue;
        }

        let mode = next_mode;
        next_mode = BoolMode::Or; // reset after consuming

        // Check for field:term syntax.
        if let Some(colon_pos) = raw_token.find(':') {
            let field = &raw_token[..colon_pos];
            let value = &raw_token[colon_pos + 1..];
            // Strip surrounding quotes from the value.
            let value = value.trim_matches('"').trim_matches('\'');
            if !field.is_empty() && !value.is_empty() {
                let field_lower = field.to_lowercase();
                // Schema and kind are identifiers — don't tokenize/stem them.
                if field_lower == "schema" || field_lower == "kind" {
                    result.push(QueryTerm {
                        field: field_lower,
                        term: value.to_lowercase(),
                        mode,
                    });
                } else {
                    // Stem the value using the same pipeline.
                    for term in tokenize(value, stemmer) {
                        result.push(QueryTerm {
                            field: field_lower.clone(),
                            term,
                            mode,
                        });
                    }
                }
                continue;
            }
        }

        // No field prefix → catch-all search.
        for term in tokenize(raw_token, stemmer) {
            result.push(QueryTerm {
                field: String::new(),
                term,
                mode,
            });
        }

        // When the raw token contains non-alphanumeric chars (e.g. "class-a",
        // "req-3080"), also emit it as a lowercased compound term. This matches
        // exact raw-ID entries in the index that tokenize() would split apart.
        let lower = raw_token.to_lowercase();
        if lower.len() >= 2 && lower.contains(|c: char| !c.is_alphanumeric()) {
            result.push(QueryTerm {
                field: String::new(),
                term: lower,
                mode,
            });
        }
    }
    result
}

pub fn query_search_index(
    indices: &[&SearchIndex],
    query: &str,
    limit: usize,
) -> Vec<SearchResult> {
    let stemmer = Stemmer::new();
    let query_terms = parse_query_terms(query, &stemmer);
    if query_terms.is_empty() {
        return Vec::new();
    }

    // Total doc count across all indices — used as IDF denominator.
    let total_doc_count: usize = indices.iter().map(|idx| idx.doc_count).sum();
    if total_doc_count == 0 {
        return Vec::new();
    }

    // Per-bid score accumulator: bid_string → (score, network_bref, title, path)
    let mut scores: HashMap<String, (f64, String, String, String)> = HashMap::new();

    /// Accumulate a TF-IDF score contribution for one (index, posting_list, penalty) triple.
    fn accumulate(
        scores: &mut HashMap<String, (f64, String, String, String)>,
        idx: &SearchIndex,
        postings: &[(String, u32)],
        penalty: f64,
        total_doc_count: usize,
    ) {
        if postings.is_empty() {
            return;
        }
        let df = postings.len() as f64;
        let idf = ((total_doc_count as f64 + 1.0) / (df + 1.0)).ln() + 1.0;
        for (bid, raw_freq) in postings {
            let Some(doc_meta) = idx.docs.get(bid.as_str()) else {
                continue;
            };
            let term_count = doc_meta.term_count.max(1) as f64;
            let tf = *raw_freq as f64 / term_count;
            let entry = scores.entry(bid.clone()).or_insert_with(|| {
                (
                    0.0,
                    idx.network_bref.clone(),
                    doc_meta.title.clone(),
                    doc_meta.path.clone(),
                )
            });
            entry.0 += tf * idf * penalty;
        }
    }

    // Separate terms by boolean mode.
    let or_terms: Vec<&QueryTerm> = query_terms
        .iter()
        .filter(|t| t.mode == BoolMode::Or)
        .collect();
    let and_terms: Vec<&QueryTerm> = query_terms
        .iter()
        .filter(|t| t.mode == BoolMode::And)
        .collect();
    let not_terms: Vec<&QueryTerm> = query_terms
        .iter()
        .filter(|t| t.mode == BoolMode::Not)
        .collect();

    // Build the index key for a query term.
    let term_key = |qt: &QueryTerm| -> String {
        if qt.field.is_empty() {
            format!("*:{}", qt.term)
        } else {
            format!("{}:{}", qt.field, qt.term)
        }
    };

    for idx in indices {
        // Score all OR terms (implicit union — same as before).
        for qt in &or_terms {
            let key = term_key(qt);
            if let Some(postings) = idx.index.get(&key) {
                accumulate(&mut scores, idx, postings, 1.0, total_doc_count);
            }
        }

        // AND terms are NOT scored — they only act as post-filters.
        // Scoring them would add documents that match only the AND term
        // (without any OR term) to the result set.

        // Fuzzy expansion for OR terms only (AND terms are boolean filters).
        let all_positive: Vec<&QueryTerm> = or_terms.to_vec();
        let needs_fuzzy = all_positive
            .iter()
            .any(|t| t.term.len() < FUZZY_MAX_QUERY_TERM_LEN);
        if needs_fuzzy {
            // Collect the set of field prefixes used by the query terms so we
            // only fuzzy-match against keys with a matching scope. Unscoped
            // queries use the catch-all "*:" prefix.
            let allowed_prefixes: std::collections::HashSet<String> = all_positive
                .iter()
                .map(|qt| {
                    if qt.field.is_empty() {
                        "*:".to_string()
                    } else {
                        format!("{}:", qt.field)
                    }
                })
                .collect();
            let index_terms: Vec<&str> = idx
                .index
                .keys()
                .filter(|k| {
                    allowed_prefixes
                        .iter()
                        .any(|pfx| k.starts_with(pfx.as_str()))
                })
                .map(|s| s.as_str())
                .collect();
            let exact_keys: std::collections::HashSet<String> =
                all_positive.iter().map(|qt| term_key(qt)).collect();
            // Build bare terms for fuzzy expansion.
            let bare_terms: Vec<String> = all_positive.iter().map(|qt| qt.term.clone()).collect();
            for (fuzzy_key, penalty) in fuzzy_expand(&bare_terms, &index_terms) {
                if exact_keys.contains(fuzzy_key) {
                    continue;
                }
                if let Some(postings) = idx.index.get(fuzzy_key) {
                    accumulate(&mut scores, idx, postings, penalty, total_doc_count);
                }
            }
        }
    }

    // Apply AND constraint: require that every AND term matched the document.
    if !and_terms.is_empty() {
        let and_keys: Vec<String> = and_terms.iter().map(|qt| term_key(qt)).collect();
        scores.retain(|bid, _| {
            and_keys.iter().all(|key| {
                indices.iter().any(|idx| {
                    idx.index
                        .get(key.as_str())
                        .is_some_and(|postings| postings.iter().any(|(b, _)| b == bid))
                })
            })
        });
    }

    // Apply NOT constraint: exclude documents matching any NOT term.
    if !not_terms.is_empty() {
        let not_keys: Vec<String> = not_terms.iter().map(|qt| term_key(qt)).collect();
        scores.retain(|bid, _| {
            !not_keys.iter().any(|key| {
                indices.iter().any(|idx| {
                    idx.index
                        .get(key.as_str())
                        .is_some_and(|postings| postings.iter().any(|(b, _)| b == bid))
                })
            })
        });
    }

    // ── Exact-ID bonus ─────────────────────────────────────────────────
    // Single-token queries (no whitespace) get a dominant bonus for exact
    // ID matches — this guarantees searching "TICKET-822" surfaces the
    // node with id="TICKET-822" at the top. Multi-token queries apply a
    // smaller bonus only for compound terms (containing non-alphanumeric
    // chars), avoiding false boosts from tokenized fragments like "ticket".
    {
        let trimmed = query.trim();
        let is_single_token = !trimmed.is_empty() && !trimmed.contains(char::is_whitespace);
        let bonus = if is_single_token { 100.0 } else { 10.0 };

        let id_keys: Vec<String> = if is_single_token {
            // Exact lookup: use the raw query as a single ID key.
            vec![format!("id:{}", trimmed.to_lowercase())]
        } else {
            // Multi-token: only boost compound terms, not stemmed fragments.
            or_terms
                .iter()
                .chain(and_terms.iter())
                .filter(|qt| qt.term.contains(|c: char| !c.is_alphanumeric()))
                .map(|qt| format!("id:{}", qt.term))
                .collect()
        };

        let mut boosted: HashSet<String> = HashSet::new();
        for idx in indices {
            for key in &id_keys {
                if let Some(postings) = idx.index.get(key.as_str()) {
                    for (bid, _freq) in postings {
                        if boosted.insert(bid.clone()) {
                            // Use entry API to ensure the node appears in
                            // results even if TF-IDF alone didn't score it
                            // (e.g. all tokens were stop-words or too short).
                            let entry = scores.entry(bid.clone()).or_insert_with(|| {
                                let dm = idx.docs.get(bid.as_str());
                                (
                                    0.0,
                                    idx.network_bref.clone(),
                                    dm.map(|d| d.title.clone()).unwrap_or_default(),
                                    dm.map(|d| d.path.clone()).unwrap_or_default(),
                                )
                            });
                            entry.0 += bonus;
                        }
                    }
                }
            }
        }
    }

    // Sort descending by score.
    let mut results: Vec<SearchResult> = scores
        .into_iter()
        .map(|(bid, (score, network_bref, title, path))| SearchResult {
            bid,
            network_bref,
            title,
            path,
            score,
        })
        .collect();
    results.sort_unstable_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if limit > 0 && results.len() > limit {
        results.truncate(limit);
    }
    results
}

#[cfg(not(target_arch = "wasm32"))]
/// Build compile-time search indices for every network in `global_bb`.
///
/// Writes:
/// - `search/manifest.json` — listing all generated indices
/// - `search/{bref}.idx.msgpack` — one per network, always
///
/// This function is called unconditionally in `finalize_html`, before the
/// sharding decision, so search indices are always present in the output.
///
/// # Arguments
///
/// * `states`     — All `BeliefNode` states from `global_bb` (borrowed, no clone)
/// * `pathmap`    — The `PathMapMap` for path resolution and network enumeration
/// * `output_dir` — The HTML output directory root
///
/// # Returns
///
/// A tuple of:
/// - [`SearchManifest`] describing all written index files
/// - `Vec<ParseDiagnostic>` containing any warnings (e.g. networks that are too large)
pub async fn build_search_indices(
    states: &FxHashMap<Bid, BeliefNode>,
    pathmap: &PathMapMap,
    repo_bid: crate::properties::Bid,
    output_dir: &Path,
) -> Result<(SearchManifest, Vec<crate::codec::ParseDiagnostic>), BuildonomyError> {
    let search_dir = output_dir.join("search");
    tokio::fs::create_dir_all(&search_dir).await?;

    let mut search_manifest = SearchManifest::new();
    let mut diagnostics: Vec<crate::codec::ParseDiagnostic> = Vec::new();

    // Construct the stemmer once — shared across all networks to avoid repeated
    // allocations. With the `stemming` feature this wraps a Snowball English
    // stemmer; without it this is a zero-cost no-op.
    let stemmer = Stemmer::new();

    // Build per-network indices using a SINGLE traversal from the repo root.
    //
    // Why single-root traversal?
    //
    // The previous approach called `pm.submap("", ...)` independently
    // for EACH network in `pathmap.nets()`. For a subnet (e.g. `subnet1`), this
    // returned paths relative to THAT subnet's root — `subnet1a/index.md` instead
    // of `subnet1/subnet1a/index.md`. The search link then navigated to the wrong
    // URL (`/subnet1a/index.html` instead of `/subnet1/subnet1a/index.html`).
    //
    // `PathMapMap::submap` called on the repo-root already traverses all
    // subnets recursively and prepends each subnet's path prefix, so every
    // returned path is repo-root-relative and correct. We then use
    // `pathmap.path(bid)` to determine which network each node belongs to and
    // partition the results into per-network `SearchIndex` instances.
    //
    // The per-network split is still useful: the viewer loads only the index for
    // the currently active network rather than a single monolithic index.
    //
    // We traverse from the REPO root PathMap (not the API/buildonomy root). The API
    // PathMap registers each network via a `network → API` Section edge whose terminal
    // path is the network's BID string (see `generate_terminal_path`). Traversing from
    // the API root therefore produces BID-prefixed paths for every document. The repo
    // network's PathMap only contains document/section paths and subnet directory paths,
    // so its `submap` returns clean repo-relative paths with no BID prefix.
    let root_bref = repo_bid.bref();

    // One SearchIndex per network bref encountered during traversal.
    let mut indices: std::collections::BTreeMap<crate::properties::Bref, SearchIndex> =
        std::collections::BTreeMap::new();

    {
        let all_paths = pathmap.submap(&root_bref, "", u8::MAX, true);
        for (path, bid, _order) in all_paths {
            // Network nodes appear twice in the submap: once as a directory
            // path (e.g. "core/data_share_sender") and once as the network
            // index file (e.g. "core/data_share_sender/CMakeLists.txt" or
            // "core/data_share_sender/index.md"). Both entries share the same
            // BID, so `index_node` would overwrite the directory entry with
            // the filename entry. The viewer navigates to the directory form
            // (normalized to `/index.html`), so the filename path produces
            // "Document Not Found". Skip the network-file entry; the
            // directory entry is the correct one.
            if is_network_index_file(Path::new(&path)) {
                continue;
            }
            let Some(node) = states.get(&bid) else {
                continue;
            };
            // Determine which network this node belongs to.
            // `PathMapMap::path` returns (home_net_bid, repo_relative_path).
            // We use our already-computed `path` (from the root traversal) which
            // is guaranteed to be repo-root-relative, and look up the home_net
            // purely for the purpose of assigning the node to the right index.
            let home_net_bref = pathmap
                .path(&bid)
                .map(|(home_net, _)| home_net.bref())
                .unwrap_or(root_bref);
            let idx = indices
                .entry(home_net_bref)
                .or_insert_with(|| SearchIndex::new(home_net_bref));
            idx.index_node(bid, node, &path, &stemmer);
        }
    }

    // Finalize and write each network's index.
    for (&net_bref, idx) in indices.iter_mut() {
        let net_bid = pathmap
            .nets()
            .iter()
            .find(|b| b.bref() == net_bref)
            .copied()
            .unwrap_or(repo_bid);

        let net_title = states
            .get(&net_bid)
            .map(|n| n.display_title())
            .unwrap_or_else(|| net_bref.to_string());

        idx.finalize();

        let idx_bytes_vec = rmp_serde::to_vec_named(idx)
            .map_err(|e| BuildonomyError::Serialization(e.to_string()))?;
        let idx_bytes = idx_bytes_vec.len();

        if idx_bytes >= LARGE_INDEX_WARN_BYTES {
            let msg = format!(
                "Network '{}' has a very large search index ({:.1} MB). \
                 Consider splitting it into smaller networks or removing \
                 low-value content to keep viewer load times fast.",
                net_title,
                idx_bytes as f64 / (1024.0 * 1024.0),
            );
            tracing::warn!("[build_search_indices] {}", msg);
            diagnostics.push(crate::codec::ParseDiagnostic::warning(msg));
        }

        let bref_str = net_bref.to_string();
        let idx_filename = format!("{}.idx.msgpack", bref_str);
        let idx_path = search_dir.join(&idx_filename);

        tokio::fs::write(&idx_path, &idx_bytes_vec).await?;

        tracing::debug!(
            "[build_search_indices] Wrote {}: {} docs, {} terms, {:.1} KB (stemmed: {:?})",
            idx_path.display(),
            idx.doc_count,
            idx.index.len(),
            idx_bytes as f64 / 1024.0,
            idx.stemmed,
        );

        search_manifest.networks.push(NetworkSearchMeta {
            bref: bref_str,
            title: net_title,
            path: idx_filename,
            size_kb: idx_bytes as f64 / 1024.0,
        });
    }

    // Write the search manifest.
    let manifest_json = serde_json::to_string_pretty(&search_manifest)
        .map_err(|e| BuildonomyError::Serialization(e.to_string()))?;
    let manifest_path = search_dir.join("manifest.json");
    tokio::fs::write(&manifest_path, manifest_json).await?;

    // Delete stale index files from previous runs whose network brefs no longer
    // exist (e.g. from ephemeral time-based BIDs that changed since the last
    // compile). Without this, the search/ directory accumulates one file per
    // unique bref ever generated, growing unboundedly.
    let current_filenames: std::collections::BTreeSet<String> = search_manifest
        .networks
        .iter()
        .map(|n| n.path.clone())
        .collect();
    if let Ok(mut read_dir) = tokio::fs::read_dir(&search_dir).await {
        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let fname = entry.file_name();
            let fname_str = fname.to_string_lossy();
            if fname_str.ends_with(".idx.msgpack")
                && !current_filenames.contains(fname_str.as_ref())
            {
                if let Err(e) = tokio::fs::remove_file(entry.path()).await {
                    tracing::warn!(
                        "[build_search_indices] Failed to remove stale index {}: {}",
                        fname_str,
                        e
                    );
                } else {
                    tracing::debug!("[build_search_indices] Removed stale index {}", fname_str);
                }
            }
        }
    }

    let total_size_kb: f64 = search_manifest.networks.iter().map(|n| n.size_kb).sum();
    tracing::debug!(
        "[build_search_indices] Generated {} network search indices, total {:.1} KB",
        search_manifest.networks.len(),
        total_size_kb,
    );

    Ok((search_manifest, diagnostics))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::properties::{BeliefKind, BeliefKindSet, NodeId};

    // ── tokenizer tests ────────────────────────────────────────────────────

    fn tok(s: &str) -> Vec<String> {
        let stemmer = Stemmer::new();
        tokenize(s, &stemmer).collect()
    }

    #[test]
    fn test_tokenize_basic() {
        let tokens = tok("Hello, World!");
        // "hello" and "world" are not stop words; they should survive.
        // With stemming they may be shortened, but both should be present.
        assert!(
            tokens
                .iter()
                .any(|t| t.starts_with("hello") || t == "hello"),
            "expected a token derived from 'hello', got: {:?}",
            tokens
        );
        assert!(
            tokens
                .iter()
                .any(|t| t.starts_with("world") || t == "world"),
            "expected a token derived from 'world', got: {:?}",
            tokens
        );
    }

    #[test]
    fn test_tokenize_lowercase() {
        let tokens = tok("BeliefBase");
        let stemmer = Stemmer::new();
        let expected = stemmer.stem("beliefbase");
        assert!(
            tokens.contains(&expected),
            "expected stem '{}', got: {:?}",
            expected,
            tokens
        );
    }

    #[test]
    fn test_tokenize_strips_short() {
        // Single-character tokens should never appear.
        let tokens = tok("a an is of the it");
        for t in &tokens {
            assert!(t.len() >= 2, "short token leaked: {:?}", t);
        }
    }

    #[test]
    fn test_tokenize_stop_words_removed() {
        // All of these are stop words and should be filtered out entirely.
        let stop_inputs = [
            "the", "a", "an", "is", "are", "and", "or", "of", "in", "on", "at", "to", "for", "it",
            "its", "be", "was", "were", "have", "has", "do", "does",
        ];
        for word in stop_inputs {
            let tokens = tok(word);
            assert!(
                tokens.is_empty(),
                "stop word '{}' leaked into index: {:?}",
                word,
                tokens
            );
        }
    }

    #[test]
    fn test_tokenize_stop_words_in_phrase() {
        // A phrase that is all stop words produces no tokens.
        let tokens = tok("the cat is on the mat");
        // "the", "is", "on", "the" are stop words; "cat" and "mat" are not.
        assert!(!tokens.is_empty(), "non-stop-words should survive");
        for t in &tokens {
            assert!(
                !stop_words().contains(t.as_str()),
                "stop word '{}' leaked: {:?}",
                t,
                tokens
            );
        }
    }

    #[test]
    fn test_tokenize_no_pure_numbers() {
        let tokens = tok("release 2024 version 42");
        assert!(!tokens.contains(&"2024".to_string()));
        assert!(!tokens.contains(&"42".to_string()));
        // "release" and "version" are not stop words; stems should appear.
        let stemmer = Stemmer::new();
        assert!(
            tokens.contains(&stemmer.stem("release")),
            "expected 'release' stem, got: {:?}",
            tokens
        );
        assert!(
            tokens.contains(&stemmer.stem("version")),
            "expected 'version' stem, got: {:?}",
            tokens
        );
    }

    #[test]
    fn test_tokenize_apostrophe_contraction() {
        // "it" is a stop word; "it's" collapses to a short/stop token.
        // "running" is not a stop word and should survive.
        let tokens = tok("it's running");
        let stemmer = Stemmer::new();
        assert!(
            tokens.contains(&stemmer.stem("running")),
            "expected 'running' stem, got: {:?}",
            tokens
        );
    }

    #[test]
    fn test_tokenize_empty() {
        assert!(tok("").is_empty());
    }

    #[test]
    fn test_tokenize_only_punctuation() {
        assert!(tok("--- ### ...").is_empty());
    }

    // ── SearchIndex unit tests ─────────────────────────────────────────────

    fn make_node(title: &str, text: &str) -> BeliefNode {
        let mut payload = toml::Table::new();
        payload.insert("text".to_string(), toml::Value::String(text.to_string()));
        BeliefNode {
            bid: Bid::new(Bid::nil()),
            kind: BeliefKindSet::from(BeliefKind::Document),
            title: title.to_string(),
            schema: None,
            payload,
            id: NodeId::default(),
            metadata: toml::Table::new(),
        }
    }

    #[test]
    fn test_index_single_node() {
        let bref = Bid::nil().bref();
        let stemmer = Stemmer::new();
        let mut idx = SearchIndex::new(bref);
        let node = make_node("Installation Guide", "how to install the software");
        let bid = node.bid;
        idx.index_node(bid, &node, "docs/install.html", &stemmer);
        idx.finalize();

        assert_eq!(idx.doc_count, 1);
        assert!(idx.docs.contains_key(&bid.to_string()));

        // With stemming: "installation" and "install" both stem to "instal" (Snowball English).
        // Without stemming: they are separate tokens.
        // We test the stem form when the feature is active, raw form otherwise.
        let bid_str = bid.to_string();

        #[cfg(feature = "stemming")]
        {
            // Both "installation" (title, ×3) and "install" (body, ×1) stem to "instal"
            // → combined freq 4 for the stem.
            let stem = stemmer.stem("installation");
            let key = format!("*:{}", stem);
            let postings = idx
                .index
                .get(&key)
                .unwrap_or_else(|| panic!("key '{}' should be indexed", key));
            let freq = postings
                .iter()
                .find(|(b, _)| b == &bid_str)
                .map(|(_, f)| *f)
                .unwrap_or(0);
            assert_eq!(freq, 4, "title stem(×3) + body stem(×1) = 4");

            let guide_stem = stemmer.stem("guide");
            let guide_key = format!("*:{}", guide_stem);
            let guide_postings = idx
                .index
                .get(&guide_key)
                .unwrap_or_else(|| panic!("key '{}' should be indexed", guide_key));
            let guide_freq = guide_postings
                .iter()
                .find(|(b, _)| b == &bid_str)
                .map(|(_, f)| *f)
                .unwrap_or(0);
            assert_eq!(guide_freq, 3, "title-only stem should have freq 3");
        }

        #[cfg(not(feature = "stemming"))]
        {
            // Without stemming: "installation" (title) and "install" (body) are separate tokens.
            let installation_postings = idx
                .index
                .get("*:installation")
                .expect("'*:installation' should be indexed (from title)");
            let installation_freq = installation_postings
                .iter()
                .find(|(b, _)| b == &bid_str)
                .map(|(_, f)| *f)
                .unwrap_or(0);
            assert_eq!(installation_freq, 3, "title-only term should have freq 3");

            let install_postings = idx
                .index
                .get("*:install")
                .expect("'*:install' should be indexed (from body)");
            let install_freq = install_postings
                .iter()
                .find(|(b, _)| b == &bid_str)
                .map(|(_, f)| *f)
                .unwrap_or(0);
            assert_eq!(install_freq, 1, "body-only term should have freq 1");

            let guide_postings = idx
                .index
                .get("*:guide")
                .expect("'*:guide' should be indexed");
            let guide_freq = guide_postings
                .iter()
                .find(|(b, _)| b == &bid_str)
                .map(|(_, f)| *f)
                .unwrap_or(0);
            assert_eq!(guide_freq, 3, "title-only term should have freq 3");
        }
    }

    #[test]
    fn test_index_skips_empty_node() {
        let bref = Bid::nil().bref();
        let stemmer = Stemmer::new();
        let mut idx = SearchIndex::new(bref);
        let node = make_node("", "");
        idx.index_node(node.bid, &node, "docs/empty.html", &stemmer);
        assert_eq!(idx.doc_count, 0);
        assert!(idx.docs.is_empty());
    }

    #[test]
    fn test_posting_list_sorted_descending() {
        let bref = Bid::nil().bref();
        let stemmer = Stemmer::new();
        let mut idx = SearchIndex::new(bref);

        // Node A: "guide" in title only → freq 3
        let node_a = make_node("Guide", "unrelated content here");
        let bid_a = node_a.bid;
        idx.index_node(bid_a, &node_a, "a.html", &stemmer);

        // Node B: "guide" in title and body → freq 3+2 = 5
        let node_b = make_node("Guide Overview", "guide guide");
        let bid_b = node_b.bid;
        idx.index_node(bid_b, &node_b, "b.html", &stemmer);

        idx.finalize();

        // Use the stemmed form of "guide" as the lookup key.
        let guide_key = format!("*:{}", stemmer.stem("guide"));
        let postings = idx
            .index
            .get(&guide_key)
            .unwrap_or_else(|| panic!("should have '{}'", guide_key));
        // First entry should have the higher frequency (node B)
        assert!(
            postings[0].1 >= postings[1].1,
            "posting list should be sorted descending by frequency"
        );
        assert_eq!(postings[0].0, bid_b.to_string());
    }

    #[test]
    fn test_index_roundtrip_serde() {
        let bref = Bid::nil().bref();
        let stemmer = Stemmer::new();
        let mut idx = SearchIndex::new(bref);
        let node = make_node("Test Document", "some test content");
        idx.index_node(node.bid, &node, "test.html", &stemmer);
        idx.finalize();

        let json = serde_json::to_string(&idx).unwrap();
        let decoded: SearchIndex = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.network_bref, Bid::nil().bref().to_string());
        assert_eq!(decoded.doc_count, 1);
        assert!(!decoded.index.is_empty());
        // Stemmed field should round-trip correctly.
        assert_eq!(decoded.stemmed, ACTIVE_STEM_MODE);
    }

    #[test]
    fn test_query_search_index_basic() {
        let bref = Bid::nil().bref();
        let stemmer = Stemmer::new();
        let mut idx = SearchIndex::new(bref);

        // Document A: about installation
        let node_a = make_node(
            "Installation Guide",
            "how to install the software correctly",
        );
        let bid_a = node_a.bid;
        idx.index_node(bid_a, &node_a, "docs/install.html", &stemmer);

        // Document B: about configuration
        let node_b = make_node("Configuration Reference", "configure settings and options");
        let bid_b = node_b.bid;
        idx.index_node(bid_b, &node_b, "docs/config.html", &stemmer);

        idx.finalize();

        // Query for a term that appears in document A only.
        let results = query_search_index(&[&idx], "install", 10);
        assert!(
            !results.is_empty(),
            "expected at least one result for 'install'"
        );

        // Results must be sorted descending by score.
        for i in 1..results.len() {
            assert!(
                results[i - 1].score >= results[i].score,
                "results not sorted descending: {:?}",
                results.iter().map(|r| r.score).collect::<Vec<_>>()
            );
        }

        // The top result should be document A (installation/install matches).
        assert_eq!(
            results[0].bid,
            bid_a.to_string(),
            "expected install doc to rank first"
        );
    }

    #[test]
    fn test_levenshtein() {
        // Exact match → distance 0
        assert_eq!(levenshtein("install", "install", 2), Some(0));
        // Single insertion → distance 1
        assert_eq!(levenshtein("sftware", "software", 2), Some(1)); // 7 vs 8 chars, one insertion
        assert_eq!(levenshtein("softwore", "software", 2), Some(1)); // single substitution
                                                                     // Two edits
        assert_eq!(levenshtein("softwa", "software", 2), Some(2));
        // Three edits — exceeds max, returns None
        assert_eq!(levenshtein("sftwa", "software", 2), None);
        // Empty strings
        assert_eq!(levenshtein("", "", 2), Some(0));
        assert_eq!(levenshtein("ab", "", 2), Some(2));
        assert_eq!(levenshtein("", "ab", 2), Some(2));
        // Length difference alone exceeds bound → fast path None
        assert_eq!(levenshtein("flght", "flight", 2), Some(1));
        assert_eq!(levenshtein("sftware", "software", 2), Some(1));
    }

    #[test]
    fn test_query_fuzzy_matching() {
        let bref = Bid::nil().bref();
        let stemmer = Stemmer::new();
        let mut idx = SearchIndex::new(bref);

        let node_a = make_node("Software Installation", "install the software package");
        let bid_a = node_a.bid;
        idx.index_node(bid_a, &node_a, "docs/install.html", &stemmer);

        let node_b = make_node("Configuration Guide", "configure your settings");
        let bid_b = node_b.bid;
        idx.index_node(bid_b, &node_b, "docs/config.html", &stemmer);

        idx.finalize();

        // "softwa" is within edit distance 2 of "software" (distance 2: drop "re").
        // It should still find the software document via fuzzy matching.
        let results = query_search_index(&[&idx], "softwa", 10);
        assert!(
            !results.is_empty(),
            "fuzzy query 'softwa' should match 'software' document; got no results. \
             Index terms: {:?}",
            idx.index.keys().collect::<Vec<_>>()
        );
        assert!(
            results.iter().any(|r| r.bid == bid_a.to_string()),
            "software document should appear in fuzzy results for 'softwa'"
        );

        // Exact matches should still outscore fuzzy matches.
        let exact_results = query_search_index(&[&idx], "software", 10);
        let fuzzy_results = query_search_index(&[&idx], "softwa", 10);
        if let (Some(exact_top), Some(fuzzy_top)) = (exact_results.first(), fuzzy_results.first()) {
            if exact_top.bid == fuzzy_top.bid {
                assert!(
                    exact_top.score >= fuzzy_top.score,
                    "exact match score ({}) should be >= fuzzy match score ({})",
                    exact_top.score,
                    fuzzy_top.score
                );
            }
        }
    }

    #[test]
    fn test_query_search_index_empty_query() {
        let bref = Bid::nil().bref();
        let stemmer = Stemmer::new();
        let mut idx = SearchIndex::new(bref);
        let node = make_node("Some Document", "some body text here");
        idx.index_node(node.bid, &node, "doc.html", &stemmer);
        idx.finalize();

        // A query with only stop words / empty should return nothing.
        let results = query_search_index(&[&idx], "the and or", 10);
        assert!(
            results.is_empty(),
            "stop-word-only query should return no results"
        );
    }

    #[test]
    fn test_stemming_merges_variants() {
        // When stemming is active, morphological variants of the same root
        // should collapse to one posting list entry with combined frequency.
        let bref = Bid::nil().bref();
        let stemmer = Stemmer::new();
        let mut idx = SearchIndex::new(bref);

        // "running" (title ×3) and "run" (body ×1) should both stem to "run".
        let node = make_node("Running Tests", "how to run the suite");
        let bid = node.bid;
        idx.index_node(bid, &node, "run.html", &stemmer);
        idx.finalize();

        #[cfg(feature = "stemming")]
        {
            let run_stem = stemmer.stem("run");
            let running_stem = stemmer.stem("running");
            // Both must produce the same stem for this test to be meaningful.
            assert_eq!(
                run_stem, running_stem,
                "Snowball English: 'run' and 'running' should share a stem"
            );

            let run_key = format!("*:{}", run_stem);
            let postings = idx
                .index
                .get(&run_key)
                .unwrap_or_else(|| panic!("key '{}' should be indexed", run_key));
            let bid_str = bid.to_string();
            let freq = postings
                .iter()
                .find(|(b, _)| b == &bid_str)
                .map(|(_, f)| *f)
                .unwrap_or(0);
            assert!(
                freq >= 4,
                "title 'running'(×3) + body 'run'(×1) should combine to ≥4, got {freq}"
            );
        }

        #[cfg(not(feature = "stemming"))]
        {
            // Without stemming "run" and "running" are separate tokens.
            // Verify at least "run" (from the body) was indexed.
            let run_key = format!("*:{}", stemmer.stem("run")); // no-op stem: returns "run"
            let postings = idx
                .index
                .get(&run_key)
                .unwrap_or_else(|| panic!("'{}' should be indexed from body", run_key));
            let bid_str = bid.to_string();
            let freq = postings
                .iter()
                .find(|(b, _)| b == &bid_str)
                .map(|(_, f)| *f)
                .unwrap_or(0);
            assert!(
                freq > 0,
                "without stemming, 'run' should still be indexed from body"
            );
        }
    }

    // ── Field-scoped search tests ──────────────────────────────────────────

    #[test]
    fn test_field_scoped_title_search() {
        let bref = Bid::nil().bref();
        let stemmer = Stemmer::new();
        let mut idx = SearchIndex::new(bref);

        let node_a = make_node("Installation Guide", "how to install the software");
        let bid_a = node_a.bid;
        idx.index_node(bid_a, &node_a, "install.html", &stemmer);

        let node_b = make_node("Configuration", "install custom packages here");
        let bid_b = node_b.bid;
        idx.index_node(bid_b, &node_b, "config.html", &stemmer);
        idx.finalize();

        let indices = vec![&idx];

        // Unscoped search: both nodes match "install".
        let results = query_search_index(&indices, "install", 0);
        assert!(
            results.len() >= 2,
            "unscoped 'install' should match both nodes; got {}",
            results.len()
        );

        // Field-scoped title search: only node_a has "installation" in the title.
        // Use the full word "installation" rather than the stem "install" so the
        // assertion holds regardless of whether the `stemming` feature is enabled.
        let results = query_search_index(&indices, "title:installation", 0);
        assert!(
            results.iter().any(|r| r.bid == bid_a.to_string()),
            "title:installation should match node_a (title='Installation Guide')"
        );
        // node_b has "Configuration" as title — no "installation" there.
        assert!(
            !results.iter().any(|r| r.bid == bid_b.to_string()),
            "title:installation should NOT match node_b (title='Configuration')"
        );
    }

    #[test]
    fn test_boolean_and() {
        let bref = Bid::nil().bref();
        let stemmer = Stemmer::new();
        let mut idx = SearchIndex::new(bref);

        let node_a = make_node("Install Guide", "how to install the software package");
        let bid_a = node_a.bid;
        idx.index_node(bid_a, &node_a, "a.html", &stemmer);

        let node_b = make_node("Package List", "available packages for download");
        let bid_b = node_b.bid;
        idx.index_node(bid_b, &node_b, "b.html", &stemmer);
        idx.finalize();

        let indices = vec![&idx];

        // "install AND package": only node_a has both terms.
        let results = query_search_index(&indices, "install AND package", 0);
        assert!(
            results.iter().any(|r| r.bid == bid_a.to_string()),
            "install AND package should match node_a"
        );
        assert!(
            !results.iter().any(|r| r.bid == bid_b.to_string()),
            "install AND package should NOT match node_b (no 'install')"
        );
    }

    #[test]
    fn test_boolean_not() {
        let bref = Bid::nil().bref();
        let stemmer = Stemmer::new();
        let mut idx = SearchIndex::new(bref);

        let node_a = make_node("Install Guide", "how to install the software package");
        let bid_a = node_a.bid;
        idx.index_node(bid_a, &node_a, "a.html", &stemmer);

        let node_b = make_node("Package List", "available packages for download");
        let bid_b = node_b.bid;
        idx.index_node(bid_b, &node_b, "b.html", &stemmer);
        idx.finalize();

        let indices = vec![&idx];

        // "package NOT install": only node_b has "package" without "install".
        let results = query_search_index(&indices, "package NOT install", 0);
        assert!(
            results.iter().any(|r| r.bid == bid_b.to_string()),
            "package NOT install should match node_b"
        );
        assert!(
            !results.iter().any(|r| r.bid == bid_a.to_string()),
            "package NOT install should NOT match node_a (has 'install')"
        );
    }

    #[test]
    fn test_unknown_field_prefix_returns_empty() {
        let bref = Bid::nil().bref();
        let stemmer = Stemmer::new();
        let mut idx = SearchIndex::new(bref);

        let node = make_node("Test Node", "some content here");
        idx.index_node(node.bid, &node, "test.html", &stemmer);
        idx.finalize();

        let indices = vec![&idx];

        // "nonexistent:test" should match nothing.
        let results = query_search_index(&indices, "nonexistent:test", 0);
        assert!(
            results.is_empty(),
            "unknown field prefix should return no results; got {}",
            results.len()
        );
    }

    #[test]
    fn test_schema_and_kind_indexed() {
        let bref = Bid::nil().bref();
        let stemmer = Stemmer::new();
        let mut idx = SearchIndex::new(bref);

        let mut node = make_node("My Requirement", "shall do something");
        node.schema = Some("requirement".to_string());
        let bid = node.bid;
        idx.index_node(bid, &node, "req.html", &stemmer);
        idx.finalize();

        let indices = vec![&idx];

        // schema:requirement should match.
        let results = query_search_index(&indices, "schema:requirement", 0);
        assert!(
            results.iter().any(|r| r.bid == bid.to_string()),
            "schema:requirement should match the node"
        );

        // kind:document should match (default kind is Document).
        let results = query_search_index(&indices, "kind:document", 0);
        assert!(
            results.iter().any(|r| r.bid == bid.to_string()),
            "kind:document should match the node"
        );
    }

    #[test]
    fn test_id_exact_match_ranks_first() {
        let bref = Bid::nil().bref();
        let stemmer = Stemmer::new();
        let mut idx = SearchIndex::new(bref);

        // Node with id="class-a" — the exact match target.
        let mut target = make_node(
            "Class A: Human Rated Space Software",
            "definition of class A",
        );
        target.id = NodeId::Explicit("class-a".to_string());
        let target_bid = target.bid;
        idx.index_node(target_bid, &target, "appendix_d.html#class-a", &stemmer);

        // Competing node that mentions "class" heavily in body text.
        let competitor = make_node(
            "Software Classification Overview",
            "class class class class class class class class software classification",
        );
        let competitor_bid = competitor.bid;
        idx.index_node(competitor_bid, &competitor, "overview.html", &stemmer);

        idx.finalize();
        let indices = vec![&idx];

        // Searching "class-a" should rank the exact ID match first.
        let results = query_search_index(&indices, "class-a", 10);
        assert!(
            !results.is_empty(),
            "search for 'class-a' should return results"
        );
        assert_eq!(
            results[0].bid,
            target_bid.to_string(),
            "exact ID match 'class-a' should rank first; got '{}' (score={}) vs target (score={})",
            results[0].title,
            results[0].score,
            results
                .iter()
                .find(|r| r.bid == target_bid.to_string())
                .map(|r| r.score)
                .unwrap_or(0.0),
        );
    }

    /// Regression test: exact ID match must rank first even when many
    /// sibling nodes share the same prefix (e.g. TICKET-100 … TICKET-900).
    ///
    /// Before the fix, `parse_query_terms("TICKET-822")` produced both a
    /// stemmed fragment `"ticket"` and the compound `"ticket-822"`. The
    /// exact-ID bonus looked up `id:ticket` in the index, which matched
    /// ALL TICKET-* nodes and boosted them equally — burying the exact
    /// match among hundreds of siblings.
    #[test]
    fn test_id_exact_match_among_siblings() {
        let bref = Bid::nil().bref();
        let stemmer = Stemmer::new();
        let mut idx = SearchIndex::new(bref);

        // The target node we want to find.
        let mut target = make_node("TICKET-822: Valve Failure", "valve failure analysis");
        target.id = NodeId::Explicit("TICKET-822".to_string());
        let target_bid = target.bid;
        idx.index_node(target_bid, &target, "ticket/822.html", &stemmer);

        // Sibling nodes that share the "TICKET-" prefix.
        for i in [100, 200, 300, 500, 700, 800, 821, 823, 900] {
            let mut node = make_node(
                &format!("TICKET-{i}: Some Hazard"),
                "hazard analysis for some subsystem failure mode",
            );
            node.id = NodeId::Explicit(format!("TICKET-{i}"));
            idx.index_node(node.bid, &node, &format!("ticket/{i}.html"), &stemmer);
        }

        idx.finalize();
        let indices = vec![&idx];

        // Searching "TICKET-822" must rank the exact ID match first.
        let results = query_search_index(&indices, "TICKET-822", 10);
        assert!(
            !results.is_empty(),
            "search for 'TICKET-822' should return results"
        );
        assert_eq!(
            results[0].bid,
            target_bid.to_string(),
            "exact ID match 'TICKET-822' should rank first; got '{}' (score={:.2})",
            results[0].title,
            results[0].score,
        );
    }
}
