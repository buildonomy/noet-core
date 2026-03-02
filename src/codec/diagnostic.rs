//! Diagnostic types for document parsing and reference resolution.
//!
//! This module provides types for tracking parsing diagnostics, particularly unresolved
//! references that need to be resolved in subsequent parse passes.
//!
//! # Position Utilities
//!
//! Use [`byte_offset_to_location`] to convert a byte offset into a source string into a
//! 1-based `(line, column)` pair suitable for human-readable diagnostic messages.

use crate::{
    nodekey::NodeKey,
    paths::AnchorPath,
    properties::{Bid, Bref, WeightKind},
};
use petgraph::Direction;
use toml_edit::Table as TomlTable;

/// Represents a reference that could not be resolved during parsing.
///
/// An unresolved reference occurs when a document references another node (via path, title, etc.)
/// but that target node is not yet available in any cache (local or global). This is a normal
/// part of multi-pass compilation and will be resolved once the target document is parsed.
///
/// # Examples
///
/// ```
/// # use noet_core::{nodekey::NodeKey, properties::{Bid, Bref, WeightKind}, codec::UnresolvedReference};
/// # use petgraph::Direction;
/// # let network_bid = Bid::default();
/// # let network_bref = network_bid.bref();
/// # let bid_of_document_a = Bid::new(network_bid);
/// // Document A references Document B before B is parsed:
/// let unresolved = UnresolvedReference {
///     direction: Direction::Outgoing,
///     self_bid: bid_of_document_a,
///     self_net: network_bid,
///     self_path: "docs/a.md".to_string(),
///     other_keys: vec![NodeKey::Path { net: network_bref, path: "docs/b.md".to_string() }],
///     weight_kind: WeightKind::Epistemic,
///     weight_data: None,
///     reference_location: Some((42, 10)), // Line 42, column 10
/// };
/// ```
#[derive(Debug, Clone)]
pub struct UnresolvedReference {
    /// Direction of the relationship from self's perspective
    pub direction: Direction,

    /// The BID of the node containing this reference
    pub self_bid: Bid,

    /// The network BID which the self_path argument is relative to.
    pub self_net: Bid,

    /// Path to the file containing this reference
    pub self_path: String,

    /// The NodeKey that could not be resolved
    pub other_keys: Vec<NodeKey>,

    /// The kind of relationship weight
    pub weight_kind: WeightKind,

    /// Optional weight data for the relationship (TomlTable for later relation creation)
    pub weight_data: Option<TomlTable>,

    /// Optional location in the source file (line, column)
    pub reference_location: Option<(usize, usize)>,
}

impl Default for UnresolvedReference {
    fn default() -> Self {
        UnresolvedReference {
            direction: Direction::Incoming,
            self_bid: Bid::nil(),
            self_net: Bid::nil(),
            self_path: String::default(),
            other_keys: vec![],
            weight_kind: WeightKind::Epistemic,
            weight_data: None,
            reference_location: None,
        }
    }
}

impl UnresolvedReference {
    /// Create a new unresolved reference
    pub fn new(
        direction: Direction,
        self_bid: Bid,
        self_net: Bid,
        self_path: String,
        other_keys: Vec<NodeKey>,
        weight_kind: WeightKind,
    ) -> Self {
        Self {
            direction,
            self_bid,
            self_net,
            self_path,
            other_keys,
            weight_kind,
            weight_data: None,
            reference_location: None,
        }
    }

    /// Create a new unresolved reference with weight data
    pub fn with_weight(
        direction: Direction,
        self_bid: Bid,
        self_net: Bid,
        self_path: String,
        other_keys: Vec<NodeKey>,
        weight_kind: WeightKind,
        weight_data: TomlTable,
    ) -> Self {
        Self {
            direction,
            self_bid,
            self_path,
            self_net,
            other_keys,
            weight_kind,
            weight_data: Some(weight_data),
            reference_location: None,
        }
    }

    /// Add location information to this unresolved reference
    pub fn with_location(mut self, line: usize, column: usize) -> Self {
        self.reference_location = Some((line, column));
        self
    }

    /// Check if this diagnostic represents a sink dependency
    pub fn is_unresolved_source(&self) -> bool {
        self.direction == Direction::Incoming
    }

    /// Get the sink path if this is a sink dependency
    pub fn as_unresolved_source(&self) -> Option<(String, Bref)> {
        if self.direction == Direction::Incoming {
            if let Some(NodeKey::Path { net, path }) = self
                .other_keys
                .iter()
                .find(|k| matches!(k, NodeKey::Path { .. }))
            {
                Some((AnchorPath::from(&path).filepath().to_string(), *net))
            } else {
                None
            }
        } else {
            None
        }
    }
}

// UnresolvedReference is the common variant during multi-pass compilation.
// Boxing would add indirection overhead to the hot path. Since diagnostics
// are already heap-allocated in Vec<ParseDiagnostic>, the size difference
// is acceptable for now. If memory usage becomes a problem, consider boxing
// large fields within UnresolvedReference (e.g., weight_data) instead of
// boxing the entire variant.
#[allow(clippy::large_enum_variant)]
/// Diagnostic information produced during document parsing.
///
/// Diagnostics represent non-fatal issues or information discovered during parsing.
/// They allow the compiler to continue processing while tracking problems that may
/// need attention or resolution in later passes.
#[derive(Debug, Clone)]
pub enum ParseDiagnostic {
    /// A reference to another node that could not be resolved
    ///
    /// This is expected during multi-pass compilation and will be resolved
    /// once the target document is parsed.
    UnresolvedReference(UnresolvedReference),

    ReparseLimitExceeded,

    /// A recoverable parse error (syntax error, IO error, etc.)
    ///
    /// The file remains in the parse queue and may be retried. Unlike fatal errors
    /// which propagate as `Err`, these are returned as part of a `ParseResult` to allow
    /// the compiler to continue processing other files.
    ParseError {
        /// Description of what went wrong
        message: String,
        /// Number of times this file has been attempted
        attempt_count: usize,
    },

    /// A warning message about the parse (e.g., deprecated syntax, ambiguous reference)
    Warning(String),

    /// An informational message about the parse
    Info(String),
}

impl ParseDiagnostic {
    /// Create a parse error diagnostic
    pub fn parse_error(message: impl Into<String>, attempt_count: usize) -> Self {
        Self::ParseError {
            message: message.into(),
            attempt_count,
        }
    }

    /// Create a warning diagnostic
    pub fn warning(message: impl Into<String>) -> Self {
        Self::Warning(message.into())
    }

    /// Create an info diagnostic
    pub fn info(message: impl Into<String>) -> Self {
        Self::Info(message.into())
    }

    /// Check if this diagnostic represents a parse error
    pub fn is_parse_error(&self) -> bool {
        matches!(self, Self::ParseError { .. })
    }

    /// Get parse error details if this is a parse error
    pub fn as_parse_error(&self) -> Option<(&str, usize)> {
        match self {
            Self::ParseError {
                message,
                attempt_count,
            } => Some((message.as_str(), *attempt_count)),
            _ => None,
        }
    }

    /// Check if this diagnostic represents an unresolved reference
    pub fn is_unresolved_reference(&self) -> bool {
        matches!(self, Self::UnresolvedReference(_))
    }

    /// Get the unresolved reference if this is one
    pub fn as_unresolved_reference(&self) -> Option<&UnresolvedReference> {
        match self {
            Self::UnresolvedReference(unresolved) => Some(unresolved),
            _ => None,
        }
    }
}

/// Convert a byte offset within a source string to a 1-based `(line, column)` pair.
///
/// Both line and column are 1-based (matching compiler convention).
/// If `offset` exceeds the length of `source`, it is clamped to `source.len()`.
///
/// # Examples
///
/// ```
/// use noet_core::codec::byte_offset_to_location;
///
/// let src = "hello\nworld\n";
/// assert_eq!(byte_offset_to_location(src, 0),  (1, 1));  // 'h'
/// assert_eq!(byte_offset_to_location(src, 5),  (1, 6));  // '\n'
/// assert_eq!(byte_offset_to_location(src, 6),  (2, 1));  // 'w'
/// assert_eq!(byte_offset_to_location(src, 11), (2, 6));  // '\n'
/// assert_eq!(byte_offset_to_location(src, 99), (3, 1));  // clamped past end
/// ```
pub fn byte_offset_to_location(source: &str, offset: usize) -> (usize, usize) {
    let clamped = offset.min(source.len());
    let before = &source[..clamped];
    let line = before.chars().filter(|&c| c == '\n').count() + 1;
    let col = before
        .rfind('\n')
        .map(|i| clamped - i - 1)
        .unwrap_or(clamped)
        + 1;
    (line, col)
}

impl std::fmt::Display for ParseDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnresolvedReference(unresolved) => {
                write!(
                    f,
                    "Unresolved reference in {:?}: {:?} -> {:?}",
                    unresolved.self_path, unresolved.self_bid, unresolved.other_keys
                )
            }
            Self::ReparseLimitExceeded => {
                write!(f, "File's reparse limit was exceeded")
            }
            Self::ParseError {
                message,
                attempt_count,
            } => write!(f, "Parse error (attempt {attempt_count}): {message}"),
            Self::Warning(msg) => write!(f, "Warning: {msg}"),
            Self::Info(msg) => write!(f, "Info: {msg}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- byte_offset_to_location tests ---

    #[test]
    fn test_byte_offset_start_of_string() {
        assert_eq!(byte_offset_to_location("hello", 0), (1, 1));
    }

    #[test]
    fn test_byte_offset_mid_line() {
        assert_eq!(byte_offset_to_location("hello world", 6), (1, 7));
    }

    #[test]
    fn test_byte_offset_at_newline() {
        // The newline character itself is still on line 1
        assert_eq!(byte_offset_to_location("hello\nworld", 5), (1, 6));
    }

    #[test]
    fn test_byte_offset_start_of_second_line() {
        assert_eq!(byte_offset_to_location("hello\nworld", 6), (2, 1));
    }

    #[test]
    fn test_byte_offset_mid_second_line() {
        assert_eq!(byte_offset_to_location("hello\nworld", 8), (2, 3));
    }

    #[test]
    fn test_byte_offset_multi_line() {
        let src = "line1\nline2\nline3";
        assert_eq!(byte_offset_to_location(src, 12), (3, 1)); // start of "line3"
        assert_eq!(byte_offset_to_location(src, 14), (3, 3)); // "ne" into "line3"
    }

    #[test]
    fn test_byte_offset_equals_len() {
        let src = "hello\n";
        // offset == len: one past the final newline, should be line 2, col 1
        assert_eq!(byte_offset_to_location(src, src.len()), (2, 1));
    }

    #[test]
    fn test_byte_offset_beyond_len_clamped() {
        let src = "hi";
        // offset > len: clamp to len (end of string, same line)
        assert_eq!(byte_offset_to_location(src, 999), (1, 3));
    }

    #[test]
    fn test_byte_offset_empty_string() {
        assert_eq!(byte_offset_to_location("", 0), (1, 1));
        assert_eq!(byte_offset_to_location("", 5), (1, 1));
    }

    #[test]
    fn test_unresolved_reference_creation() {
        let unresolved = UnresolvedReference::new(
            Direction::Outgoing,
            Bid::nil(),
            Bid::nil(),
            "test.md".to_string(),
            vec![NodeKey::Path {
                net: Bref::default(),
                path: "other.md".to_string(),
            }],
            WeightKind::Epistemic,
        );

        assert_eq!(unresolved.direction, Direction::Outgoing);
        assert!(unresolved.weight_data.is_none());
        assert!(unresolved.reference_location.is_none());
    }

    #[test]
    fn test_unresolved_reference_with_location() {
        let unresolved = UnresolvedReference::new(
            Direction::Outgoing,
            Bid::nil(),
            Bid::nil(),
            "test.md".to_string(),
            vec![NodeKey::Path {
                net: Bref::default(),
                path: "other.md".to_string(),
            }],
            WeightKind::Epistemic,
        )
        .with_location(42, 10);

        assert_eq!(unresolved.reference_location, Some((42, 10)));
    }

    #[test]
    fn test_parse_diagnostic_creation() {
        let warning = ParseDiagnostic::warning("Test warning");
        let info = ParseDiagnostic::info("Test info");
        let parse_error = ParseDiagnostic::parse_error("Syntax error", 2);

        assert!(matches!(warning, ParseDiagnostic::Warning(_)));
        assert!(matches!(info, ParseDiagnostic::Info(_)));
        assert!(parse_error.is_parse_error());
        assert_eq!(parse_error.as_parse_error().unwrap().1, 2);
    }

    #[test]
    fn test_parse_diagnostic_is_unresolved() {
        let unresolved = ParseDiagnostic::UnresolvedReference(UnresolvedReference::new(
            Direction::Outgoing,
            Bid::nil(),
            Bid::nil(),
            "test.md".to_string(),
            vec![NodeKey::Path {
                net: Bref::default(),
                path: "other.md".to_string(),
            }],
            WeightKind::Epistemic,
        ));

        assert!(unresolved.is_unresolved_reference());
        assert!(unresolved.as_unresolved_reference().is_some());

        let warning = ParseDiagnostic::warning("test");
        assert!(!warning.is_unresolved_reference());
        assert!(warning.as_unresolved_reference().is_none());
    }
}
