//! Compile-time search index building.
//!
//! Generates per-network `search/{bref}.idx.json` files during `finalize_html`.
//! These files are always produced — regardless of whether data sharding is
//! active — so the viewer can search the entire corpus from the moment it loads,
//! even before any data shard is fetched.
//!
//! ## Index Format
//!
//! Each `.idx.json` file is a compact inverted index for one network:
//!
//! ```json
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
//! - Issue 54: Full-Text Search MVP (deserializes and queries the files in WASM)

use crate::{
    error::BuildonomyError,
    paths::PathMapMap,
    properties::{BeliefNode, Bid, Bref},
    shard::manifest::{NetworkSearchMeta, SearchManifest},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// Warn when a single network's search index exceeds this size (bytes).
///
/// A large index is a proxy for a large network. Authors should consider
/// splitting the network or removing low-value content.
/// 5MB index → roughly 100–150MB of source text.
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
            "not",
            "only",
            "whether",
            "although",
            "because",
            "since",
            "unless",
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
            "within",
            "without",
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
            "will",
            "would",
            "shall",
            "should",
            "may",
            "might",
            "must",
            "can",
            "could",
            "get",
            "got",
            "let",
            // Common adverbs / discourse markers
            "no",
            "yes",
            "not",
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
            "all",
            "any",
            "each",
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
            "always",
            "never",
            "ever",
            "often",
            "however",
            "therefore",
            "thus",
            "hence",
            "else",
            "if",
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
const TITLE_WEIGHT: u32 = 3;

/// The active stem mode for this build.
///
/// When the `stemming` feature is enabled this is [`StemMode::English`];
/// otherwise [`StemMode::None`]. Used to populate [`SearchIndex::stemmed`].
#[cfg(feature = "stemming")]
const ACTIVE_STEM_MODE: StemMode = StemMode::English;
#[cfg(not(feature = "stemming"))]
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
}

/// Compile-time inverted index for a single network.
///
/// Serialized to `search/{bref}.idx.json` during `finalize_html`. The WASM
/// side (Issue 54) deserializes this and runs TF-IDF queries against it —
/// no index construction happens in the browser.
///
/// See `docs/design/search_and_sharding.md` §7.2 for the JSON schema.
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
    /// Inverted index: `term → [(bid_string, frequency)]`.
    ///
    /// Frequencies for title terms are pre-multiplied by `TITLE_WEIGHT`.
    /// Entries within each posting list are sorted descending by frequency for
    /// fast top-K retrieval.
    pub index: BTreeMap<String, Vec<(String, u32)>>,
}

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

        if title.is_empty() && body_text.is_empty() {
            return;
        }

        let bid_str = bid.to_string();

        // Accumulate term → frequency for this document.
        let mut term_freqs: BTreeMap<String, u32> = BTreeMap::new();

        // Index title terms with boosted weight.
        for term in tokenize(&title, stemmer) {
            *term_freqs.entry(term).or_insert(0) += TITLE_WEIGHT;
        }

        // Index body terms with unit weight.
        for term in tokenize(&body_text, stemmer) {
            *term_freqs.entry(term).or_insert(0) += 1;
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
            postings.sort_unstable_by(|a, b| b.1.cmp(&a.1));
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

/// Build compile-time search indices for every network in `global_bb`.
///
/// Writes:
/// - `search/manifest.json` — listing all generated indices
/// - `search/{bref}.idx.json` — one per network, always
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
    states: &BTreeMap<Bid, BeliefNode>,
    pathmap: &PathMapMap,
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

    // Iterate over every network in the PathMapMap.
    // `nets()` returns the set of network BIDs registered with the pathmap.
    for &net_bid in pathmap.nets() {
        let net_bref = net_bid.bref();

        // Retrieve the network node title.
        let net_title = states
            .get(&net_bid)
            .map(|n| n.display_title())
            .unwrap_or_else(|| net_bref.to_string());

        // Build a search index for this network.
        let mut idx = SearchIndex::new(net_bref);

        // Enumerate all nodes that belong to this network via the PathMapMap.
        // `PathMapMap::get_map(bref)` returns the PathMap for one network, which
        // contains `(path_string, bid, sort_order)` entries for every node.
        if let Some(pm) = pathmap.get_map(&net_bref) {
            // We iterate the full recursive map to include subnets' documents too.
            let all_paths = pm.recursive_map(pathmap, &mut std::collections::BTreeSet::new());
            for (path, bid, _order) in all_paths {
                if let Some(node) = states.get(&bid) {
                    idx.index_node(bid, node, &path, &stemmer);
                }
            }
        }

        idx.finalize();

        // Serialize the index.
        let idx_json = serde_json::to_string(&idx)
            .map_err(|e| BuildonomyError::Serialization(e.to_string()))?;
        let idx_bytes = idx_json.len();

        // Warn when a network's search index is suspiciously large — this is a
        // proxy for a network that has grown too large and should be split.
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
        let idx_filename = format!("{}.idx.json", bref_str);
        let idx_path = search_dir.join(&idx_filename);

        tokio::fs::write(&idx_path, &idx_json).await?;

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

    let total_size_kb: f64 = search_manifest.networks.iter().map(|n| n.size_kb).sum();
    tracing::info!(
        "[build_search_indices] Generated {} network search indices, total {:.1} KB",
        search_manifest.networks.len(),
        total_size_kb,
    );

    Ok((search_manifest, diagnostics))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        use crate::properties::{BeliefKind, BeliefKindSet};
        let mut payload = toml::Table::new();
        payload.insert("text".to_string(), toml::Value::String(text.to_string()));
        BeliefNode {
            bid: Bid::new(Bid::nil()),
            kind: BeliefKindSet::from(BeliefKind::Document),
            title: title.to_string(),
            schema: None,
            payload,
            id: None,
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
            let postings = idx
                .index
                .get(&stem)
                .unwrap_or_else(|| panic!("stem '{}' should be indexed", stem));
            let freq = postings
                .iter()
                .find(|(b, _)| b == &bid_str)
                .map(|(_, f)| *f)
                .unwrap_or(0);
            assert_eq!(freq, 4, "title stem(×3) + body stem(×1) = 4");

            let guide_stem = stemmer.stem("guide");
            let guide_postings = idx
                .index
                .get(&guide_stem)
                .unwrap_or_else(|| panic!("stem '{}' should be indexed", guide_stem));
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
                .get("installation")
                .expect("'installation' should be indexed (from title)");
            let installation_freq = installation_postings
                .iter()
                .find(|(b, _)| b == &bid_str)
                .map(|(_, f)| *f)
                .unwrap_or(0);
            assert_eq!(installation_freq, 3, "title-only term should have freq 3");

            let install_postings = idx
                .index
                .get("install")
                .expect("'install' should be indexed (from body)");
            let install_freq = install_postings
                .iter()
                .find(|(b, _)| b == &bid_str)
                .map(|(_, f)| *f)
                .unwrap_or(0);
            assert_eq!(install_freq, 1, "body-only term should have freq 1");

            let guide_postings = idx.index.get("guide").expect("'guide' should be indexed");
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
        let guide_key = stemmer.stem("guide");
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
    fn test_index_roundtrip_json() {
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

            let postings = idx
                .index
                .get(&run_stem)
                .unwrap_or_else(|| panic!("stem '{}' should be indexed", run_stem));
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
            let run_key = stemmer.stem("run"); // no-op: returns "run"
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
}
