// query/parser.rs — Textual query grammar parser and serializer.
//
// Parses the noet query language (see docs/design/query_model.md §9.5) into
// a `QuerySpec` and serializes `QuerySpec` back to canonical text.
//
// ## Grammar summary (no view-config suffix — that travels as sibling URL params)
//
//   query        = pipeline (comp_op pipeline)*
//   comp_op      = 'AND' | 'OR' | 'NOT'
//   pipeline     = [seed] stage (tape_fn? stage)*
//   seed         = anchor | seed_fn
//   anchor       = ('id://' | 'id:' | 'bref:' | 'bid:') WORD
//   seed_fn      = 'KEYS' '(' nodekey (',' nodekey)* ')'
//                | 'CORPUS' '(' ')'
//                | 'BIDS' '(' bid (',' bid)* ')'
//   stage        = traversal | filter_expr
//   traversal    = full_traversal | '->' shorthand | '<-' kind ['(' depth ')']
//   full_trav    = role_set '-' kind_set '-' role_set ['(' depth_spec ')'] ['?']
//   filter_expr  = filter_or
//   filter_or    = filter_and ('OR' filter_and)*
//   filter_and   = filter_not ('AND' filter_not)*
//   filter_not   = 'NOT' filter_atom | filter_atom
//   filter_atom  = '(' filter_expr ')' | simple_filter
//   simple_filter = predicate | text_match
//
// Composition (AND/OR/NOT) between pipelines uses `StepOperation::Compose`.
// Boolean AND/OR within a filter_expr also uses Compose with Filter branches.
// NOT as a filter prefix wraps the atom in Compose(pass_all, Not, atom).

use std::fmt::Write as FmtWrite;
use std::str::FromStr;

use enumset::EnumSet;

use crate::nodekey::NodeKey;
use crate::properties::WeightKind;
use crate::query::spec::{
    leaves, parse_property_path, roots, CompareOp, Composition, CompositionOp, DepthCount,
    EdgePredicate, NodeFilter, ProjectionStep, PropertyPath, PropertyPredicate, PropertySegment,
    PropertyValue, QuerySpec, Role, SetOp, StepOperation, StepRef, TapeFn, TraversalDepth,
    TraversalSpec,
};

// ═════════════════════════════════════════════════════════════════════════════
// ParseError
// ═════════════════════════════════════════════════════════════════════════════

/// Error produced by the query parser.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("parse error at offset {offset}: {message}")]
pub struct ParseError {
    /// Byte offset in the input string where the error was detected.
    pub offset: usize,
    /// Human-readable description of the problem.
    pub message: String,
}

impl ParseError {
    fn new(offset: usize, message: impl Into<String>) -> Self {
        Self {
            offset,
            message: message.into(),
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Token
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// Any run of non-special characters (alphanumeric, `.`, `_`, `/`, `~`, `+`, `%`, `@`).
    Word(String),
    /// `"..."` — content without the quotes.
    Quoted(String),
    /// `id://WORD` — the whole anchor string including the scheme.
    IdAnchor(String),
    Eq,         // ==
    NotEq,      // !=
    Gt,         // >
    Lt,         // <
    Gte,        // >=
    Lte,        // <=
    Dash,       // -
    ArrowRight, // ->
    ArrowLeft,  // <-
    LParen,     // (
    RParen,     // )
    Comma,      // ,
    Star,       // *
    Bang,       // !
    Eof,
}

// ═════════════════════════════════════════════════════════════════════════════
// Lexer — pre-tokenise entire input into Vec<(Token, usize)>
// ═════════════════════════════════════════════════════════════════════════════

/// Normalise a raw word token to its canonical keyword form.
///
/// Operator keywords (`AND`, `OR`, `NOT`, `THEN`, `FOLD`, `TERMINAL`, `ORPHAN`,
/// set ops `UNION` etc.) are case-insensitive and normalised to uppercase.
/// Predicate operators (`in`, `matches`, `contains`, `exists`) are normalised
/// to lowercase. All other words pass through unchanged.
///
/// This is the single point at which case-folding happens. The rest of the
/// parser compares against the canonical forms only.
///
/// **Why this is unambiguous in practice**: the TF-IDF search engine already
/// strips conjunctions and other low-value terms as stop words before indexing.
/// A text search for the bare word "and" or "or" would score zero and return
/// no useful results anyway. Treating them as operators (which is what any user
/// actually intends when they type them in a query) is therefore unambiguously
/// correct. Quoted literals (`"and"`) still reach the search engine as-is for
/// the rare case where exact-string matching is needed.
fn normalize_keyword(w: &str) -> String {
    match w.to_ascii_lowercase().as_str() {
        "and" => "AND".into(),
        "or" => "OR".into(),
        "not" => "NOT".into(),
        "then" => "THEN".into(),
        "fold" => "FOLD".into(),
        "terminal" => "TERMINAL".into(),
        "orphan" => "ORPHAN".into(),
        "union" => "UNION".into(),
        "intersect" => "INTERSECT".into(),
        "ldiff" => "LDIFF".into(),
        "rdiff" => "RDIFF".into(),
        "symdiff" => "SYMDIFF".into(),
        // Predicate operators — canonical lowercase
        // Seed function keywords
        "keys" => "KEYS".into(),
        "corpus" => "CORPUS".into(),
        "bids" => "BIDS".into(),
        "in" | "matches" | "contains" | "exists" => w.to_ascii_lowercase(),
        _ => w.to_string(),
    }
}

/// Returns the reserved characters that cannot appear in a bare `Word` token.
fn is_word_char(c: char) -> bool {
    !matches!(
        c,
        ' ' | '\t'
            | '\n'
            | '\r'
            | ':'
            | '('
            | ')'
            | ','
            | '|'
            | '$'
            | '?'
            | '*'
            | '"'
            | '!'
            | '<'
            | '>'
            | '='
            | '-'
            | '#'
    )
}

fn tokenise(input: &str) -> Result<Vec<(Token, usize)>, ParseError> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut pos = 0;
    let mut tokens: Vec<(Token, usize)> = Vec::new();

    while pos < len {
        // Skip whitespace
        if bytes[pos].is_ascii_whitespace() {
            pos += 1;
            continue;
        }

        let start = pos;
        let ch = input[pos..].chars().next().unwrap();

        // ── Anchor schemes: id://WORD, bref:WORD, bid:WORD ─────────────────────
        // Recognizes both hierarchical (id://value) and non-hierarchical
        // (bref:value, bid:value) anchor formats. All produce IdAnchor tokens.
        {
            let anchor_prefix = if input[pos..].starts_with("id://") {
                Some(5)
            } else if input[pos..].starts_with("id:") {
                Some(3)
            } else if input[pos..].starts_with("bref:") {
                Some(5)
            } else if input[pos..].starts_with("bid:") {
                Some(4)
            } else {
                None
            };
            if let Some(prefix_len) = anchor_prefix {
                pos += prefix_len;
                let word_start = pos;
                // Anchor words allow hyphens in addition to normal word chars,
                // since bref slugs and IDs commonly contain hyphens.
                while pos < len {
                    let c = input[pos..].chars().next().unwrap();
                    if !is_word_char(c) && c != '-' {
                        break;
                    }
                    pos += c.len_utf8();
                }
                if pos == word_start {
                    return Err(ParseError::new(
                        start,
                        format!(
                            "expected word after '{}'",
                            &input[start..start + prefix_len]
                        ),
                    ));
                }
                tokens.push((Token::IdAnchor(input[start..pos].to_string()), start));
                continue;
            }
        }

        // ── Multi-char operators ──────────────────────────────────────────
        if pos + 1 < len {
            let two = &input[pos..pos + 2];
            match two {
                "->" => {
                    tokens.push((Token::ArrowRight, start));
                    pos += 2;
                    continue;
                }
                "<-" => {
                    tokens.push((Token::ArrowLeft, start));
                    pos += 2;
                    continue;
                }
                "==" => {
                    tokens.push((Token::Eq, start));
                    pos += 2;
                    continue;
                }
                "!=" => {
                    tokens.push((Token::NotEq, start));
                    pos += 2;
                    continue;
                }
                ">=" => {
                    tokens.push((Token::Gte, start));
                    pos += 2;
                    continue;
                }
                "<=" => {
                    tokens.push((Token::Lte, start));
                    pos += 2;
                    continue;
                }
                _ => {}
            }
        }

        // ── Single-char tokens ────────────────────────────────────────────
        match ch {
            '-' => {
                tokens.push((Token::Dash, start));
                pos += 1;
                continue;
            }
            '(' => {
                tokens.push((Token::LParen, start));
                pos += 1;
                continue;
            }
            ')' => {
                tokens.push((Token::RParen, start));
                pos += 1;
                continue;
            }
            ',' => {
                tokens.push((Token::Comma, start));
                pos += 1;
                continue;
            }

            '*' => {
                tokens.push((Token::Star, start));
                pos += 1;
                continue;
            }
            '!' if pos + 1 >= len || bytes[pos + 1] != b'=' => {
                // Bare `!` (not `!=`)
                tokens.push((Token::Bang, start));
                pos += 1;
                continue;
            }
            '>' => {
                tokens.push((Token::Gt, start));
                pos += 1;
                continue;
            }
            '<' => {
                tokens.push((Token::Lt, start));
                pos += 1;
                continue;
            }
            '"' => {
                pos += 1; // skip opening quote
                let s_start = pos;
                while pos < len && bytes[pos] != b'"' {
                    if bytes[pos] == b'\\' {
                        pos += 1;
                    } // skip escape
                    pos += 1;
                }
                if pos >= len {
                    return Err(ParseError::new(start, "unterminated quoted string"));
                }
                let s = input[s_start..pos].to_string();
                pos += 1; // skip closing quote
                tokens.push((Token::Quoted(s), start));
                continue;
            }
            '|' | '$' | '#' | '!' | '=' => {
                // Reserved but not a recognised two-char sequence
                return Err(ParseError::new(
                    start,
                    format!("unexpected character '{ch}'"),
                ));
            }
            _ => {}
        }

        // ── Word ──────────────────────────────────────────────────────────
        if is_word_char(ch) {
            let word_start = pos;
            while pos < len {
                let c = input[pos..].chars().next().unwrap();
                if !is_word_char(c) {
                    break;
                }
                pos += c.len_utf8();
            }
            // Check for trailing colon (field:term) — colon is NOT a word char;
            // we emit the word first, then the colon is handled as a separate token.
            // But first, check if the word is followed immediately by ':' and that
            // pattern is the field-search syntax — we handle that in the parser.
            //
            // Colon IS a reserved char, so we just emit the word here and the
            // `:` will cause an error if it appears unexpected — the parser handles
            // `word ':'` as a special sequence in parse_simple_filter.
            //
            // Actually: `:` is in the reserved set, so we need to handle it.
            // Re-examine: the word ends here; the next char `:` would be a separate
            // token. But `:` is in the reserved set and not handled above.
            // We need to add `:` as a valid token.
            tokens.push((
                Token::Word(normalize_keyword(&input[word_start..pos])),
                word_start,
            ));
            continue;
        }

        // `:` — used for field:term and edge_filter syntax. Emitted as a
        // distinguishable Word(":") token so the parser can handle field:term
        // without making colon a first-class token type.
        if ch == ':' {
            tokens.push((Token::Word(":".to_string()), start));
            pos += 1;
            continue;
        }

        return Err(ParseError::new(
            start,
            format!("unexpected character '{ch}'"),
        ));
    }

    tokens.push((Token::Eof, len));
    Ok(tokens)
}

// ═════════════════════════════════════════════════════════════════════════════
// Parser
// ═════════════════════════════════════════════════════════════════════════════

struct Parser {
    tokens: Vec<(Token, usize)>,
    pos: usize,
    input_len: usize,
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn is_role_set(w: &str) -> bool {
    !w.is_empty() && w.chars().all(|c| matches!(c, 's' | 'k' | 'o' | 'n'))
}

/// Named traversal shorthands.
///
/// The verb names (`composed_of`, `component_of`, `uses`, …) are the canonical names
/// from `crate::codec::myst::DIRECTIVES` entries with `ref_role.is_some()`, plus the
/// `covers` block-directive synonym and the structural constructors from `spec.rs`.
///
/// **SYNC REQUIRED**: when adding or renaming verb directives in
/// `src/codec/myst.rs::DIRECTIVES`, update this list and `parse_named_shorthand` below.
/// `src/codec/myst.rs` carries a reciprocal note pointing here.
fn is_shorthand_name(w: &str) -> bool {
    matches!(
        w,
        // Section verbs (DIRECTIVES: composed_of ref_role=Source, component_of ref_role=Sink)
        // consists_of is a backward-compatible alias for composed_of
        "composed_of" | "consists_of" | "component_of"
        // Pragmatic verbs (DIRECTIVES: uses/implements ref_role=Source, used_by ref_role=Sink)
        // plus covers (block-directive synonym for maps_to, owner traversal)
        | "uses" | "implements" | "used_by" | "covers"
        // Epistemic / normative verbs (DIRECTIVES: constrained_by/draws_from ref_role=Source,
        // constrains/underlies ref_role=Sink)
        | "constrained_by" | "constrains" | "draws_from" | "underlies"
        // Structural — from spec.rs roots()/leaves() and TraversalSpec::halo()
        | "roots" | "leaves" | "halo"
    )
}

fn is_comp_op(w: &str) -> bool {
    matches!(w, "AND" | "OR" | "NOT")
}

fn is_tape_fn_keyword(w: &str) -> bool {
    matches!(w, "THEN" | "FOLD" | "TERMINAL" | "ORPHAN")
}

fn is_seed_fn_keyword(w: &str) -> bool {
    matches!(w, "KEYS" | "CORPUS" | "BIDS")
}

fn parse_kind(w: &str) -> Option<WeightKind> {
    match w {
        "section" => Some(WeightKind::Section),
        "epistemic" => Some(WeightKind::Epistemic),
        "pragmatic" => Some(WeightKind::Pragmatic),
        _ => None,
    }
}

fn kind_name(k: WeightKind) -> &'static str {
    match k {
        WeightKind::Section => "section",
        WeightKind::Epistemic => "epistemic",
        WeightKind::Pragmatic => "pragmatic",
    }
}

fn roles_from_str(s: &str) -> EnumSet<Role> {
    let mut set = EnumSet::empty();
    for c in s.chars() {
        match c {
            's' => {
                set |= Role::Source;
            }
            'k' => {
                set |= Role::Sink;
            }
            'o' => {
                set |= Role::Owner;
            }
            'n' => {
                set |= Role::Source | Role::Sink | Role::Owner;
            }
            _ => {}
        }
    }
    set
}

fn pass_all_filter() -> NodeFilter {
    NodeFilter::Predicate(PropertyPredicate {
        path: parse_property_path("kind").expect("static path"),
        op: CompareOp::Exists,
        value: PropertyValue::None,
    })
}

/// Path for the `text:` multi-field alias (searches all indexed text fields).
fn text_path() -> PropertyPath {
    parse_property_path("text").expect("static path")
}

// ── Parser impl ───────────────────────────────────────────────────────────────

impl Parser {
    fn new(tokens: Vec<(Token, usize)>, input_len: usize) -> Self {
        Self {
            tokens,
            pos: 0,
            input_len,
        }
    }

    fn peek_at(&self, offset: usize) -> &Token {
        self.tokens
            .get(self.pos + offset)
            .map(|(t, _)| t)
            .unwrap_or(&Token::Eof)
    }

    fn offset_at(&self, offset: usize) -> usize {
        self.tokens
            .get(self.pos + offset)
            .map(|(_, o)| *o)
            .unwrap_or(self.input_len)
    }

    fn current_offset(&self) -> usize {
        self.offset_at(0)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].0.clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    #[allow(dead_code)]
    fn expect_word(&mut self, expected: &str) -> Result<(), ParseError> {
        match self.peek_at(0) {
            Token::Word(w) if w == expected => {
                self.advance();
                Ok(())
            }
            other => Err(ParseError::new(
                self.current_offset(),
                format!("expected '{expected}', found {other:?}"),
            )),
        }
    }

    fn expect_token(&mut self, expected: &Token) -> Result<(), ParseError> {
        if self.peek_at(0) == expected {
            self.advance();
            Ok(())
        } else {
            Err(ParseError::new(
                self.current_offset(),
                format!("expected {expected:?}, found {:?}", self.peek_at(0)),
            ))
        }
    }

    fn peek_word_is(&self, w: &str) -> bool {
        matches!(self.peek_at(0), Token::Word(s) if s == w)
    }

    #[allow(dead_code)]
    fn peek_at_word_is(&self, offset: usize, w: &str) -> bool {
        matches!(self.peek_at(offset), Token::Word(s) if s == w)
    }

    /// Can the token at `offset` start a filter_atom?
    fn can_start_filter_atom_at(&self, offset: usize) -> bool {
        match self.peek_at(offset) {
            Token::LParen | Token::Quoted(_) => true,
            Token::Word(w) => {
                if is_comp_op(w) || is_tape_fn_keyword(w) {
                    return false;
                }
                if w == "NOT" {
                    return self.can_start_filter_atom_at(offset + 1);
                }
                // Role-set followed by dash = full traversal, not a filter atom
                if is_role_set(w) && matches!(self.peek_at(offset + 1), Token::Dash) {
                    return false;
                }
                // Named shorthand followed by paren = traversal, not a filter atom
                if is_shorthand_name(w) && matches!(self.peek_at(offset + 1), Token::LParen) {
                    return false;
                }
                true
            }
            Token::IdAnchor(_) | Token::ArrowRight | Token::ArrowLeft | Token::Eof => false,
            _ => false,
        }
    }

    fn peek_is_traversal_start(&self) -> bool {
        self.peek_is_traversal_start_at(0)
    }

    fn peek_is_traversal_start_at(&self, offset: usize) -> bool {
        match self.peek_at(offset) {
            // Arrows kept for backward-compat error messages only
            Token::ArrowRight | Token::ArrowLeft => true,
            // `!` prefix: inverted traversal (existence gate negation)
            Token::Bang => self.peek_is_traversal_start_at(offset + 1),
            Token::Word(w) => {
                // Full traversal: role_set followed by dash
                if is_role_set(w) && matches!(self.peek_at(offset + 1), Token::Dash) {
                    return true;
                }
                // Named shorthand: verb followed by open paren
                if is_shorthand_name(w) && matches!(self.peek_at(offset + 1), Token::LParen) {
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    fn peek_is_stage_start(&self) -> bool {
        match self.peek_at(0) {
            Token::Eof | Token::RParen => false,
            // Anchors (bid:, bref:, id://) are seeds, not stages.
            Token::IdAnchor(_) => false,
            Token::Word(w) => !is_comp_op(w) && !is_tape_fn_keyword(w) && !is_seed_fn_keyword(w),
            _ => true,
        }
    }

    // ── Top-level ──────────────────────────────────────────────────────────

    fn parse_query(&mut self) -> Result<QuerySpec, ParseError> {
        let (seed1, mut steps) = self.parse_comp_or()?;
        self.parse_continuation_stages(&mut steps)?;

        // Build the final spec with the seed on the first step
        let mut spec = match seed1 {
            Some(seed) => QuerySpec::seed_then(seed, steps),
            None => QuerySpec::new(steps),
        };
        label_steps(&mut spec.steps);
        Ok(spec)
    }

    /// Parse continuation stages: TAPE_FN and/or bare stages that follow
    /// an initial stage or composition. Used by `parse_query` (top level),
    /// `parse_comp_atom` (inside grouping parens), and `parse_pipeline`
    /// (multi-stage pipelines).
    ///
    /// A `TAPE_FN` without a following stage produces an Identity step,
    /// allowing terminal folds like `composed_of(*) FOLD(UNION)` and
    /// chained tape functions like `FOLD(UNION) THEN used_by(1)`.
    fn parse_continuation_stages(
        &mut self,
        steps: &mut Vec<ProjectionStep>,
    ) -> Result<(), ParseError> {
        loop {
            let tape_fn_opt = self.parse_tape_fn_if_present()?;
            let had_tape_fn = tape_fn_opt.is_some();
            let tape_fn = tape_fn_opt.unwrap_or(TapeFn::Then(None));

            if had_tape_fn {
                if !self.peek_is_stage_start() {
                    // Terminal tape function with no following stage —
                    // emit an Identity step so the fold/select takes effect.
                    steps.push(ProjectionStep::with_input(tape_fn, StepOperation::Identity));
                    continue;
                }
            } else if !self.peek_is_stage_start()
                || matches!(self.peek_at(0), Token::Word(w) if is_comp_op(w))
            {
                break;
            }

            let mut stage = self.parse_stage()?;
            if let Some(first) = stage.first_mut() {
                first.input = tape_fn;
            }
            steps.extend(stage);
        }
        Ok(())
    }

    // ── Composition precedence: OR (lowest) > AND > NOT (highest) ──────

    /// Parse a composition expression at the OR (lowest precedence) level.
    /// OR is left-associative: `A OR B OR C` = `(A OR B) OR C`.
    fn parse_comp_or(&mut self) -> Result<(Option<TapeFn>, Vec<ProjectionStep>), ParseError> {
        let (mut seed, mut left_steps) = self.parse_comp_and()?;

        while self.peek_word_is("OR") {
            self.advance(); // consume OR
            let (seed2, right_steps) = self.parse_comp_and()?;

            let left = inject_seed_into_branch(seed, left_steps);
            let right = inject_seed_into_branch(seed2, right_steps);

            let compose_step = ProjectionStep::compose(Composition {
                left,
                op: CompositionOp::Or,
                right,
            });
            left_steps = vec![compose_step];
            // Seed was consumed by inject_seed_into_branch — don't propagate it
            seed = None;
        }

        Ok((seed, left_steps))
    }

    /// Parse a composition expression at the AND/NOT (medium precedence) level.
    /// Both AND and NOT are left-associative binary infix operators at this level.
    /// `A AND B AND C` = `(A AND B) AND C`.
    /// `A NOT B` = `A NOT B` (left diff / exclusion).
    /// `A AND B NOT C` = `(A AND B) NOT C`.
    ///
    /// NOT also exists as a unary prefix at a higher precedence level
    /// (`parse_comp_not`). The binary infix NOT here handles `A NOT B`
    /// backward-compat; the unary prefix NOT handles `NOT B` (= corpus \ B).
    fn parse_comp_and(&mut self) -> Result<(Option<TapeFn>, Vec<ProjectionStep>), ParseError> {
        let (mut seed, mut left_steps) = self.parse_comp_not()?;

        while (self.peek_word_is("AND") || self.peek_word_is("NOT"))
            && self.peek_is_comp_atom_start_at(1)
        {
            let op = if self.peek_word_is("NOT") {
                self.advance();
                CompositionOp::Not
            } else {
                self.advance();
                CompositionOp::And
            };
            let (seed2, right_steps) = self.parse_comp_not()?;

            let left = inject_seed_into_branch(seed, left_steps);
            let right = inject_seed_into_branch(seed2, right_steps);

            let compose_step = ProjectionStep::compose(Composition { left, op, right });
            left_steps = vec![compose_step];
            // Seed was consumed by inject_seed_into_branch — don't propagate it
            seed = None;
        }

        Ok((seed, left_steps))
    }

    /// Parse a composition expression at the NOT (highest precedence) level.
    ///
    /// At this level, NOT is only unary prefix: `NOT pipeline` = `pass_all NOT pipeline`
    /// (corpus \ pipeline). Binary infix NOT (`A NOT B` = A \ B) is handled at
    /// the AND level by `parse_comp_and`, which consumes both AND and NOT as
    /// binary operators at the same precedence.
    fn parse_comp_not(&mut self) -> Result<(Option<TapeFn>, Vec<ProjectionStep>), ParseError> {
        if self.peek_word_is("NOT") && self.peek_is_comp_not_start_at(1) {
            self.advance(); // consume NOT
            let (_seed, atom_steps) = self.parse_comp_not()?;
            let right = inject_seed_into_branch(_seed, atom_steps);
            Ok((
                None,
                vec![ProjectionStep::compose(Composition {
                    left: vec![ProjectionStep::filter(pass_all_filter())],
                    op: CompositionOp::Not,
                    right,
                })],
            ))
        } else {
            self.parse_comp_atom()
        }
    }

    /// Parse a composition atom: either a parenthesized composition group
    /// or a bare pipeline.
    fn parse_comp_atom(&mut self) -> Result<(Option<TapeFn>, Vec<ProjectionStep>), ParseError> {
        if matches!(self.peek_at(0), Token::LParen) && self.peek_is_comp_group_start() {
            self.advance(); // consume (
            let (_seed, mut steps) = self.parse_comp_or()?;
            // Inject the seed inside the group — it doesn't escape
            steps = inject_seed_into_branch(_seed, steps);
            // Post-composition continuation inside the group:
            // `((A NOT B) THEN C)` = compose A NOT B, then pipe through C.
            self.parse_continuation_stages(&mut steps)?;
            self.expect_token(&Token::RParen)?;
            Ok((None, steps))
        } else {
            self.parse_pipeline()
        }
    }

    /// Check whether a `(` at the current position starts a composition
    /// group (as opposed to argument parens, which are always consumed
    /// inside stages/seeds). At this level, `(` must be a grouping paren
    /// because argument parens are consumed at lower levels (inside
    /// `parse_stage`, `parse_seed_if_present`, etc.).
    ///
    /// We need to distinguish from the case where `(` starts a filter atom
    /// inside a pipeline — but that's fine because `parse_pipeline` →
    /// `parse_stage` → `parse_filter_atom` handles filter-level `(` internally.
    /// At the comp_atom level, `(` followed by something that looks like a
    /// pipeline start (seed, stage, or another `(`) means grouping.
    fn peek_is_comp_group_start(&self) -> bool {
        // Look past the `(` to see if the next token could start a pipeline
        // or composition expression. If so, this is a grouping paren.
        match self.peek_at(1) {
            Token::Eof | Token::RParen => false,
            // A seed anchor after ( means this is a grouped pipeline
            Token::IdAnchor(_) => true,
            // NOT after ( means a grouped NOT expression
            Token::Word(w) if w == "NOT" => true,
            // A seed keyword after ( means a grouped pipeline
            Token::Word(w) if is_seed_fn_keyword(w) => true,
            // Another ( means nested grouping
            Token::LParen => true,
            // Any other word that's not a comp_op could start a stage
            Token::Word(w) => !is_comp_op(w) && !is_tape_fn_keyword(w),
            _ => true,
        }
    }

    /// Check whether the token at offset `n` from current position could
    /// start a composition atom (pipeline or grouped expression).
    fn peek_is_comp_atom_start_at(&self, n: usize) -> bool {
        match self.peek_at(n) {
            Token::Eof | Token::RParen => false,
            Token::IdAnchor(_) => true,
            Token::LParen => true,
            Token::Word(w) => !is_comp_op(w) && !is_tape_fn_keyword(w),
            _ => true,
        }
    }

    /// Like `peek_is_comp_atom_start_at` but also accepts `NOT` (for
    /// chained unary NOT: `NOT NOT title:x`).
    fn peek_is_comp_not_start_at(&self, n: usize) -> bool {
        if matches!(self.peek_at(n), Token::Word(w) if w == "NOT") {
            return true;
        }
        self.peek_is_comp_atom_start_at(n)
    }

    /// Parse an optional seed at the current position: bare anchor token or
    /// explicit seed function (`KEYS(...)`, `CORPUS()`, `BIDS(...)`).
    fn parse_seed_if_present(&mut self) -> Result<Option<TapeFn>, ParseError> {
        // Bare anchor: id://WORD, bref:WORD, bid:WORD
        if let Token::IdAnchor(_) = self.peek_at(0) {
            if let Token::IdAnchor(s) = self.advance() {
                let key = NodeKey::from_str(&s)
                    .map_err(|e| ParseError::new(0, format!("invalid anchor '{s}': {e}")))?;
                return Ok(Some(TapeFn::Keys(vec![key])));
            }
        }

        // Explicit seed functions: KEYS(...), CORPUS(), BIDS(...)
        match self.peek_at(0) {
            Token::Word(w) if w == "KEYS" && matches!(self.peek_at(1), Token::LParen) => {
                self.advance(); // consume KEYS
                self.expect_token(&Token::LParen)?;
                let keys = self.parse_nodekey_list()?;
                self.expect_token(&Token::RParen)?;
                if keys.is_empty() {
                    return Err(ParseError::new(
                        self.current_offset(),
                        "KEYS() requires at least one argument",
                    ));
                }
                Ok(Some(TapeFn::Keys(keys)))
            }
            Token::Word(w) if w == "CORPUS" && matches!(self.peek_at(1), Token::LParen) => {
                self.advance(); // consume CORPUS
                self.expect_token(&Token::LParen)?;
                self.expect_token(&Token::RParen)?;
                Ok(Some(TapeFn::Corpus))
            }
            Token::Word(w) if w == "BIDS" && matches!(self.peek_at(1), Token::LParen) => {
                self.advance(); // consume BIDS
                self.expect_token(&Token::LParen)?;
                let bids = self.parse_bid_list()?;
                self.expect_token(&Token::RParen)?;
                if bids.is_empty() {
                    return Err(ParseError::new(
                        self.current_offset(),
                        "BIDS() requires at least one argument",
                    ));
                }
                Ok(Some(TapeFn::Bids(bids)))
            }
            _ => Ok(None),
        }
    }

    /// Parse a comma-separated list of NodeKey strings (for `KEYS(...)`).
    /// Each item is an anchor-like token or bare word parsed via `NodeKey::from_str`.
    fn parse_nodekey_list(&mut self) -> Result<Vec<NodeKey>, ParseError> {
        let mut keys = Vec::new();
        loop {
            // Collect the key string from the next token(s)
            let offset = self.current_offset();
            let key_str = match self.peek_at(0) {
                Token::IdAnchor(_) => {
                    if let Token::IdAnchor(s) = self.advance() {
                        s
                    } else {
                        unreachable!()
                    }
                }
                Token::Word(_) => {
                    if let Token::Word(w) = self.advance() {
                        w
                    } else {
                        unreachable!()
                    }
                }
                _ => break,
            };
            let key = NodeKey::from_str(&key_str).map_err(|e| {
                ParseError::new(offset, format!("invalid node key '{key_str}': {e}"))
            })?;
            keys.push(key);
            if matches!(self.peek_at(0), Token::Comma) {
                self.advance(); // consume comma
            } else {
                break;
            }
        }
        Ok(keys)
    }

    /// Parse a comma-separated list of BID strings (for `BIDS(...)`).
    fn parse_bid_list(&mut self) -> Result<Vec<crate::properties::Bid>, ParseError> {
        let mut bids = Vec::new();
        loop {
            let offset = self.current_offset();
            let bid_str = match self.peek_at(0) {
                Token::Word(_) => {
                    if let Token::Word(w) = self.advance() {
                        w
                    } else {
                        unreachable!()
                    }
                }
                _ => break,
            };
            let bid = crate::properties::Bid::try_from(bid_str.as_str())
                .map_err(|e| ParseError::new(offset, format!("invalid BID '{bid_str}': {e}")))?;
            bids.push(bid);
            if matches!(self.peek_at(0), Token::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        Ok(bids)
    }

    fn parse_pipeline(&mut self) -> Result<(Option<TapeFn>, Vec<ProjectionStep>), ParseError> {
        // Optional seed: bare anchor or explicit seed function
        let seed = self.parse_seed_if_present()?;

        if !self.peek_is_stage_start() {
            if seed.is_some() {
                // Bare seed with no stages → seed-only pipeline (Identity).
                return Ok((seed, vec![]));
            }
            return Err(ParseError::new(
                self.current_offset(),
                "expected a query stage (traversal or filter expression)",
            ));
        }

        let mut steps: Vec<ProjectionStep> = self.parse_stage()?;
        self.parse_continuation_stages(&mut steps)?;

        Ok((seed, steps))
    }

    fn parse_tape_fn_if_present(&mut self) -> Result<Option<TapeFn>, ParseError> {
        match self.peek_at(0) {
            Token::Word(w) if w == "THEN" => {
                self.advance();
                let step_ref = self.parse_optional_step_ref()?;
                Ok(Some(TapeFn::Then(step_ref)))
            }
            Token::Word(w) if w == "FOLD" => {
                self.advance();
                self.expect_token(&Token::LParen)?;
                let op = self.parse_set_op()?;
                let range = if matches!(self.peek_at(0), Token::Comma) {
                    self.advance();
                    let a = self.parse_step_ref()?;
                    self.expect_token(&Token::Comma)?;
                    let b = self.parse_step_ref()?;
                    Some((a, b))
                } else {
                    None
                };
                self.expect_token(&Token::RParen)?;
                Ok(Some(TapeFn::Fold { op, range }))
            }
            Token::Word(w) if w == "TERMINAL" => {
                self.advance();
                let range = self.parse_optional_range()?;
                Ok(Some(TapeFn::Terminal(range)))
            }
            Token::Word(w) if w == "ORPHAN" => {
                self.advance();
                let range = self.parse_optional_range()?;
                Ok(Some(TapeFn::Orphan(range)))
            }
            _ => Ok(None),
        }
    }

    fn parse_optional_step_ref(&mut self) -> Result<Option<StepRef>, ParseError> {
        if matches!(self.peek_at(0), Token::LParen) {
            self.advance();
            let r = self.parse_step_ref()?;
            self.expect_token(&Token::RParen)?;
            Ok(Some(r))
        } else {
            Ok(None)
        }
    }

    fn parse_optional_range(&mut self) -> Result<Option<(StepRef, StepRef)>, ParseError> {
        if matches!(self.peek_at(0), Token::LParen) {
            self.advance();
            let a = self.parse_step_ref()?;
            self.expect_token(&Token::Comma)?;
            let b = self.parse_step_ref()?;
            self.expect_token(&Token::RParen)?;
            Ok(Some((a, b)))
        } else {
            Ok(None)
        }
    }

    fn parse_step_ref(&mut self) -> Result<StepRef, ParseError> {
        match self.peek_at(0) {
            Token::Word(w) => {
                let w = w.clone();
                self.advance();
                if let Ok(n) = w.parse::<usize>() {
                    Ok(StepRef::Index(n))
                } else {
                    Ok(StepRef::Label(w))
                }
            }
            other => Err(ParseError::new(
                self.current_offset(),
                format!("expected step reference, found {other:?}"),
            )),
        }
    }

    fn parse_set_op(&mut self) -> Result<SetOp, ParseError> {
        match self.peek_at(0) {
            Token::Word(w) => {
                let op = match w.as_str() {
                    "UNION"     => SetOp::Union,
                    "INTERSECT" => SetOp::Intersection,
                    "LDIFF"     => SetOp::LeftDiff,
                    "RDIFF"     => SetOp::RightDiff,
                    "SYMDIFF"   => SetOp::SymmetricDiff,
                    other => return Err(ParseError::new(
                        self.current_offset(),
                        format!("unknown set operation '{other}'; expected UNION, INTERSECT, LDIFF, RDIFF, or SYMDIFF"),
                    )),
                };
                self.advance();
                Ok(op)
            }
            other => Err(ParseError::new(
                self.current_offset(),
                format!("expected set operation, found {other:?}"),
            )),
        }
    }

    // ── Stage dispatch ─────────────────────────────────────────────────────

    fn parse_stage(&mut self) -> Result<Vec<ProjectionStep>, ParseError> {
        if self.peek_is_traversal_start() {
            self.parse_traversal_stage()
        } else {
            self.parse_filter_stage()
        }
    }

    // ── Traversal ──────────────────────────────────────────────────

    fn parse_traversal_stage(&mut self) -> Result<Vec<ProjectionStep>, ParseError> {
        // Consume optional `!` prefix for inverted traversals
        let inverted = if matches!(self.peek_at(0), Token::Bang) {
            self.advance();
            true
        } else {
            false
        };

        let mut steps = match self.peek_at(0) {
            Token::ArrowRight | Token::ArrowLeft => {
                let offset = self.current_offset();
                return Err(ParseError::new(
                    offset,
                    "use named shorthands: composed_of(N), component_of(N), \
                     uses(N), used_by(N), constrained_by(N), constrains(N), covers(N), \
                     roots(), leaves(), halo()",
                ));
            }
            Token::Word(w) if is_shorthand_name(w) => self.parse_named_shorthand()?,
            _ => vec![self.parse_full_traversal()?],
        };

        if inverted {
            for step in &mut steps {
                if let StepOperation::Traverse(ref mut t) = step.operation {
                    t.inverted = true;
                }
            }
        }

        Ok(steps)
    }

    fn parse_full_traversal(&mut self) -> Result<ProjectionStep, ParseError> {
        let role_word = match self.advance() {
            Token::Word(w) => w,
            other => {
                return Err(ParseError::new(
                    self.current_offset(),
                    format!("expected role set, found {other:?}"),
                ))
            }
        };
        let input_roles = roles_from_str(&role_word);

        self.expect_token(&Token::Dash)?;
        let kind_filter = self.parse_kind_set()?;
        self.expect_token(&Token::Dash)?;

        let out_word = match self.advance() {
            Token::Word(w) => w,
            other => {
                return Err(ParseError::new(
                    self.current_offset(),
                    format!("expected output role set, found {other:?}"),
                ))
            }
        };
        let output_roles = roles_from_str(&out_word);

        // Degenerate self-loop check: only reject single-role self-loops
        // (e.g. s-...-s, k-...-k). Multi-role expressions like n-...-n are
        // valid — they resolve ALL neighbors in any role, producing new nodes.
        if input_roles.len() == 1 && input_roles == output_roles {
            return Err(ParseError::new(
                self.current_offset(),
                format!("degenerate traversal: single-role self-loop ({role_word}-\u{2026}-{out_word}) always returns the input node; did you mean ->pragmatic(1) or a different output role?"),
            ));
        }

        let depth = self.parse_depth_spec_if_present()?;

        Ok(ProjectionStep::traverse(TraversalSpec {
            input_roles,
            kind_filter,
            output_roles,
            depth,
            inverted: false,
        }))
    }

    // ── Named shorthands ───────────────────────────────────────────────

    /// Parse a named traversal shorthand: `verb(N)` or `verb()`.
    ///
    /// Verbs are derived from DIRECTIVES verb names and TraversalSpec constructors:
    ///
    /// | Verb | Expansion | Reading |
    /// |------|-----------|---------|
    /// | `composed_of(N)` | `k-section-s(N)` | root→leaf (submap) |
    /// | `consists_of(N)` | (alias for `composed_of`) | |
    /// | `component_of(N)` | `s-section-k(N)` | leaf→root |
    /// | `uses(N)` / `implements(N)` | `k-pragmatic-s(N)` | what this depends on |
    /// | `used_by(N)` | `s-pragmatic-k(N)` | what depends on this |
    /// | `constrained_by(N)` | `k-epistemic-s(N)` | normative constraints on this (preferred) |
    /// | `constrains(N)` | `s-epistemic-k(N)` | what this normatively constrains (preferred) |
    /// | `draws_from(N)` | `k-epistemic-s(N)` | alias for constrained_by |
    /// | `underlies(N)` | `s-epistemic-k(N)` | alias for constrains |
    /// | `covers(N)` | `o-epistemic-sk(N)` | MapsTo: owner→edge endpoints |
    /// | `roots()` | `s-section-k(*) TERMINAL` | all root nodes |
    /// | `leaves()` | `k-section-s(*) TERMINAL` | all leaf nodes |
    /// | `halo()` | `n-*-n(1)` | immediate full neighborhood |
    fn parse_named_shorthand(&mut self) -> Result<Vec<ProjectionStep>, ParseError> {
        let offset = self.current_offset();
        let name = match self.advance() {
            Token::Word(w) => w,
            _ => unreachable!(),
        };

        // roots(), leaves(), halo() — accept `()` or `(*)`; depth is fixed
        match name.as_str() {
            "roots" => {
                self.consume_empty_or_star_parens(&name)?;
                return Ok(roots());
            }
            "leaves" => {
                self.consume_empty_or_star_parens(&name)?;
                return Ok(leaves());
            }
            "halo" => {
                self.consume_empty_or_star_parens(&name)?;
                return Ok(vec![ProjectionStep::traverse(TraversalSpec::halo())]);
            }
            _ => {}
        }

        // All others accept an explicit depth: verb(N) or verb()
        let depth = if matches!(self.peek_at(0), Token::LParen) {
            self.parse_depth_spec_if_present()?
        } else {
            return Err(ParseError::new(
                offset,
                format!("`{name}` requires a depth argument, e.g. `{name}(1)` or `{name}(*)`"),
            ));
        };

        let step = match name.as_str() {
            "composed_of" | "consists_of" => ProjectionStep::traverse(traversal(
                Role::Sink.into(),
                WeightKind::Section.into(),
                Role::Source.into(),
                depth,
            )),
            "component_of" => ProjectionStep::traverse(traversal(
                Role::Source.into(),
                WeightKind::Section.into(),
                Role::Sink.into(),
                depth,
            )),
            "uses" | "implements" => ProjectionStep::traverse(traversal(
                Role::Sink.into(),
                WeightKind::Pragmatic.into(),
                Role::Source.into(),
                depth,
            )),
            "used_by" => ProjectionStep::traverse(traversal(
                Role::Source.into(),
                WeightKind::Pragmatic.into(),
                Role::Sink.into(),
                depth,
            )),
            "constrained_by" | "draws_from" => ProjectionStep::traverse(traversal(
                Role::Sink.into(),
                WeightKind::Epistemic.into(),
                Role::Source.into(),
                depth,
            )),
            "constrains" | "underlies" => ProjectionStep::traverse(traversal(
                Role::Source.into(),
                WeightKind::Epistemic.into(),
                Role::Sink.into(),
                depth,
            )),
            "covers" => ProjectionStep::traverse(traversal(
                Role::Owner.into(),
                WeightKind::Epistemic.into(),
                Role::Source | Role::Sink,
                depth,
            )),
            other => {
                return Err(ParseError::new(
                    offset,
                    format!("unknown shorthand '{other}'"),
                ))
            }
        };
        Ok(vec![step])
    }

    /// Consume `()` or `(*)` for shorthands with fixed depth (roots, leaves, halo).
    fn consume_empty_or_star_parens(&mut self, name: &str) -> Result<(), ParseError> {
        let offset = self.current_offset();
        if !matches!(self.peek_at(0), Token::LParen) {
            return Err(ParseError::new(
                offset,
                format!("`{name}` requires parentheses, e.g. `{name}()`"),
            ));
        }
        self.advance(); // consume (
                        // Accept empty () or (*) — depth is always fixed for these shorthands
        if matches!(self.peek_at(0), Token::Star) {
            self.advance(); // consume *
        }
        self.expect_token(&Token::RParen)?;
        Ok(())
    }

    fn parse_kind_set(&mut self) -> Result<EnumSet<WeightKind>, ParseError> {
        if matches!(self.peek_at(0), Token::Star) {
            self.advance();
            return Ok(EnumSet::all());
        }
        let mut kinds = EnumSet::empty();
        loop {
            match self.peek_at(0) {
                Token::Word(w) => {
                    let w = w.clone();
                    if let Some(k) = parse_kind(&w) {
                        self.advance();
                        kinds |= k;
                    } else {
                        return Err(ParseError::new(
                            self.current_offset(),
                            format!(
                                "expected kind (section/epistemic/pragmatic) or '*', found '{w}'"
                            ),
                        ));
                    }
                }
                other => {
                    return Err(ParseError::new(
                        self.current_offset(),
                        format!("expected kind, found {other:?}"),
                    ))
                }
            }
            if matches!(self.peek_at(0), Token::Comma) {
                // peek ahead to see if next is another kind
                if matches!(self.peek_at(1), Token::Word(w) if parse_kind(w).is_some()) {
                    self.advance(); // consume comma
                    continue;
                }
            }
            break;
        }
        Ok(kinds)
    }

    fn parse_depth_spec_if_present(&mut self) -> Result<TraversalDepth, ParseError> {
        if !matches!(self.peek_at(0), Token::LParen) {
            return Ok(TraversalDepth::count(1));
        }
        self.advance(); // consume (

        // Determine count vs edge_filter
        let depth = match self.peek_at(0) {
            Token::Star => {
                self.advance();
                let edge_filter = self.parse_comma_edge_filter()?;
                TraversalDepth {
                    count: DepthCount::Max,
                    edge_filter,
                }
            }
            Token::Word(w) if w.chars().all(|c| c.is_ascii_digit()) => {
                let n_str = w.clone();
                self.advance();
                let n: u8 = n_str.parse().map_err(|_| {
                    ParseError::new(
                        self.current_offset(),
                        format!("depth count '{n_str}' out of range (0-255)"),
                    )
                })?;
                let edge_filter = self.parse_comma_edge_filter()?;
                TraversalDepth {
                    count: DepthCount::N(n),
                    edge_filter,
                }
            }
            _ => {
                // edge_filter only → implies count = 1
                let edge_filter = Some(self.parse_edge_filter()?);
                TraversalDepth {
                    count: DepthCount::N(1),
                    edge_filter,
                }
            }
        };

        self.expect_token(&Token::RParen)?;
        Ok(depth)
    }

    fn parse_comma_edge_filter(&mut self) -> Result<Option<EdgePredicate>, ParseError> {
        if matches!(self.peek_at(0), Token::Comma) {
            self.advance();
            Ok(Some(self.parse_edge_filter()?))
        } else {
            Ok(None)
        }
    }

    fn parse_edge_filter(&mut self) -> Result<EdgePredicate, ParseError> {
        // Expect: Word ':' Word  (property:value)
        // Note: ':' is emitted as Word(":") in our lexer
        let prop = match self.advance() {
            Token::Word(w) => w,
            other => {
                return Err(ParseError::new(
                    self.current_offset(),
                    format!("expected edge filter property name, found {other:?}"),
                ))
            }
        };
        // consume ':'
        match self.peek_at(0) {
            Token::Word(w) if w == ":" => {
                self.advance();
            }
            other => {
                return Err(ParseError::new(
                    self.current_offset(),
                    format!("expected ':' in edge filter, found {other:?}"),
                ))
            }
        }
        let val = match self.advance() {
            Token::Word(w) | Token::Quoted(w) => w,
            other => {
                return Err(ParseError::new(
                    self.current_offset(),
                    format!("expected edge filter value, found {other:?}"),
                ))
            }
        };
        let path = parse_property_path(&prop)
            .map_err(|e| ParseError::new(self.current_offset(), e.to_string()))?;
        // Try to parse as number for idx filters
        let value = if let Ok(n) = val.parse::<f64>() {
            PropertyValue::Number(n)
        } else {
            PropertyValue::String(val)
        };
        Ok(EdgePredicate {
            path,
            op: CompareOp::Eq,
            value,
        })
    }

    // ── Filter expressions ─────────────────────────────────────────────────

    fn parse_filter_stage(&mut self) -> Result<Vec<ProjectionStep>, ParseError> {
        self.parse_filter_or()
    }

    fn parse_filter_or(&mut self) -> Result<Vec<ProjectionStep>, ParseError> {
        let mut steps = self.parse_filter_and()?;
        while self.peek_word_is("OR") && self.can_start_filter_atom_at(1) {
            self.advance(); // consume OR
            let right = self.parse_filter_and()?;
            steps = vec![ProjectionStep::compose(Composition {
                left: steps,
                op: CompositionOp::Or,
                right,
            })];
        }
        Ok(steps)
    }

    fn parse_filter_and(&mut self) -> Result<Vec<ProjectionStep>, ParseError> {
        let mut steps = self.parse_filter_not()?;
        while self.peek_word_is("AND") && self.can_start_filter_atom_at(1) {
            self.advance(); // consume AND
            let right = self.parse_filter_not()?;
            steps = vec![ProjectionStep::compose(Composition {
                left: steps,
                op: CompositionOp::And,
                right,
            })];
        }
        Ok(steps)
    }

    fn parse_filter_not(&mut self) -> Result<Vec<ProjectionStep>, ParseError> {
        if self.peek_word_is("NOT") && self.can_start_filter_atom_at(1) {
            self.advance(); // consume NOT
            let atom = self.parse_filter_atom()?;
            Ok(vec![ProjectionStep::compose(Composition {
                left: vec![ProjectionStep::filter(pass_all_filter())],
                op: CompositionOp::Not,
                right: atom,
            })])
        } else {
            self.parse_filter_atom()
        }
    }

    fn parse_filter_atom(&mut self) -> Result<Vec<ProjectionStep>, ParseError> {
        if matches!(self.peek_at(0), Token::LParen) {
            self.advance(); // consume (
            let steps = self.parse_filter_or()?;
            self.expect_token(&Token::RParen)?;
            return Ok(steps);
        }
        // Bare colon shorthand: `:term` or `:"multi word"` expands to `text:term`.
        // The `:` sigil alone means "search all indexed text fields".
        if matches!(self.peek_at(0), Token::Word(w) if w == ":") {
            let offset = self.current_offset();
            self.advance(); // consume ':'
            let term = match self.advance() {
                Token::Word(t) | Token::Quoted(t) => t,
                other => {
                    return Err(ParseError::new(
                        offset,
                        format!("expected search term after ':', found {other:?}"),
                    ))
                }
            };
            return Ok(vec![ProjectionStep::filter(NodeFilter::TextMatch {
                path: text_path(),
                query: term,
            })]);
        }
        self.parse_simple_filter()
    }

    fn parse_simple_filter(&mut self) -> Result<Vec<ProjectionStep>, ParseError> {
        let offset = self.current_offset();

        // Only Word tokens start a filter expression. Bare quoted strings and
        // bare words without a field prefix are no longer accepted — TextMatch
        // always requires an explicit `field:term` or `field:"multi word"` form.
        let word = match self.peek_at(0) {
            Token::Word(w) => {
                let w = w.clone();
                self.advance();
                w
            }
            Token::Quoted(_) => {
                return Err(ParseError::new(
                    offset,
                    "bare quoted string is not a valid filter expression; \
                     use field:term syntax, e.g. text:\"foo bar\""
                        .to_string(),
                ))
            }
            other => {
                return Err(ParseError::new(
                    offset,
                    format!("expected filter expression, found {other:?}"),
                ))
            }
        };

        // Keywords are normalised to canonical form by the lexer (normalize_keyword),
        // so "and"/"AND"/"And" all arrive here as "AND" and are handled as operators.
        // No guard needed — a bare operator word in filter position is simply an operator.

        // Disambiguate by what follows
        match self.peek_at(0) {
            // Symbolic operators → property predicate
            Token::Eq => {
                self.advance();
                let val = self.parse_string_value()?;
                let path = parse_property_path(&word)
                    .map_err(|e| ParseError::new(offset, e.to_string()))?;
                Ok(vec![ProjectionStep::filter(NodeFilter::Predicate(
                    PropertyPredicate {
                        path,
                        op: CompareOp::Eq,
                        value: val,
                    },
                ))])
            }
            Token::NotEq => {
                self.advance();
                let val = self.parse_string_value()?;
                let path = parse_property_path(&word)
                    .map_err(|e| ParseError::new(offset, e.to_string()))?;
                Ok(vec![ProjectionStep::filter(NodeFilter::Predicate(
                    PropertyPredicate {
                        path,
                        op: CompareOp::NotEq,
                        value: val,
                    },
                ))])
            }
            Token::Gt => {
                self.advance();
                let val = self.parse_number_value()?;
                let path = parse_property_path(&word)
                    .map_err(|e| ParseError::new(offset, e.to_string()))?;
                Ok(vec![ProjectionStep::filter(NodeFilter::Predicate(
                    PropertyPredicate {
                        path,
                        op: CompareOp::Gt,
                        value: val,
                    },
                ))])
            }
            Token::Lt => {
                self.advance();
                let val = self.parse_number_value()?;
                let path = parse_property_path(&word)
                    .map_err(|e| ParseError::new(offset, e.to_string()))?;
                Ok(vec![ProjectionStep::filter(NodeFilter::Predicate(
                    PropertyPredicate {
                        path,
                        op: CompareOp::Lt,
                        value: val,
                    },
                ))])
            }
            Token::Gte => {
                self.advance();
                let val = self.parse_number_value()?;
                let path = parse_property_path(&word)
                    .map_err(|e| ParseError::new(offset, e.to_string()))?;
                Ok(vec![ProjectionStep::filter(NodeFilter::Predicate(
                    PropertyPredicate {
                        path,
                        op: CompareOp::Gte,
                        value: val,
                    },
                ))])
            }
            Token::Lte => {
                self.advance();
                let val = self.parse_number_value()?;
                let path = parse_property_path(&word)
                    .map_err(|e| ParseError::new(offset, e.to_string()))?;
                Ok(vec![ProjectionStep::filter(NodeFilter::Predicate(
                    PropertyPredicate {
                        path,
                        op: CompareOp::Lte,
                        value: val,
                    },
                ))])
            }
            // Word operators
            Token::Word(op) if op == "in" => {
                self.advance(); // consume 'in'
                self.expect_token(&Token::LParen)?;
                let mut set = Vec::new();
                loop {
                    match self.advance() {
                        Token::Word(v) | Token::Quoted(v) => set.push(v),
                        other => {
                            return Err(ParseError::new(
                                self.current_offset(),
                                format!("expected value in set, found {other:?}"),
                            ))
                        }
                    }
                    match self.peek_at(0) {
                        Token::Comma => {
                            self.advance();
                        }
                        Token::RParen => {
                            self.advance();
                            break;
                        }
                        other => {
                            return Err(ParseError::new(
                                self.current_offset(),
                                format!("expected ',' or ')' in set, found {other:?}"),
                            ))
                        }
                    }
                }
                let path = parse_property_path(&word)
                    .map_err(|e| ParseError::new(offset, e.to_string()))?;
                Ok(vec![ProjectionStep::filter(NodeFilter::Predicate(
                    PropertyPredicate {
                        path,
                        op: CompareOp::In,
                        value: PropertyValue::Set(set),
                    },
                ))])
            }
            Token::Word(op) if op == "matches" => {
                self.advance();
                let pat = match self.advance() {
                    Token::Quoted(s) | Token::Word(s) => s,
                    other => {
                        return Err(ParseError::new(
                            self.current_offset(),
                            format!("expected regex pattern after 'matches', found {other:?}"),
                        ))
                    }
                };
                let path = parse_property_path(&word)
                    .map_err(|e| ParseError::new(offset, e.to_string()))?;
                Ok(vec![ProjectionStep::filter(NodeFilter::Predicate(
                    PropertyPredicate {
                        path,
                        op: CompareOp::Matches,
                        value: PropertyValue::Regex(pat),
                    },
                ))])
            }
            Token::Word(op) if op == "contains" => {
                self.advance();
                let val = self.parse_string_value()?;
                let path = parse_property_path(&word)
                    .map_err(|e| ParseError::new(offset, e.to_string()))?;
                Ok(vec![ProjectionStep::filter(NodeFilter::Predicate(
                    PropertyPredicate {
                        path,
                        op: CompareOp::Contains,
                        value: val,
                    },
                ))])
            }
            Token::Word(op) if op == "exists" => {
                self.advance();
                let path = parse_property_path(&word)
                    .map_err(|e| ParseError::new(offset, e.to_string()))?;
                Ok(vec![ProjectionStep::filter(NodeFilter::Predicate(
                    PropertyPredicate {
                        path,
                        op: CompareOp::Exists,
                        value: PropertyValue::None,
                    },
                ))])
            }
            // Colon → TextMatch on the named field/path.
            // Any property path is valid; the evaluator resolves what it means.
            // Single-word terms may be unquoted; multi-word terms must be quoted.
            Token::Word(c) if c == ":" => {
                self.advance(); // consume ':'
                let term = match self.advance() {
                    Token::Word(t) | Token::Quoted(t) => t,
                    other => {
                        return Err(ParseError::new(
                            self.current_offset(),
                            format!("expected search term after ':', found {other:?}"),
                        ))
                    }
                };
                let path = parse_property_path(&word)
                    .map_err(|e| ParseError::new(offset, e.to_string()))?;
                Ok(vec![ProjectionStep::filter(NodeFilter::TextMatch {
                    path,
                    query: term,
                })])
            }
            // Anything else: bare word with no field prefix or operator is a parse error.
            // TextMatch always requires an explicit field prefix (e.g. text:word).
            other => Err(ParseError::new(
                offset,
                format!(
                    "bare word '{word}' is not a valid filter expression; \
                     to search text use field:term syntax (e.g. text:{word}), \
                     or use a predicate operator (e.g. {word} == value), \
                     found {other:?} after '{word}'"
                ),
            )),
        }
    }

    fn parse_string_value(&mut self) -> Result<PropertyValue, ParseError> {
        match self.advance() {
            Token::Quoted(s) | Token::Word(s) => Ok(PropertyValue::String(s)),
            other => Err(ParseError::new(
                self.current_offset(),
                format!("expected string value, found {other:?}"),
            )),
        }
    }

    fn parse_number_value(&mut self) -> Result<PropertyValue, ParseError> {
        match self.advance() {
            Token::Word(s) => {
                let n: f64 = s.parse().map_err(|_| {
                    ParseError::new(
                        self.current_offset(),
                        format!("expected numeric value, found '{s}'"),
                    )
                })?;
                Ok(PropertyValue::Number(n))
            }
            other => Err(ParseError::new(
                self.current_offset(),
                format!("expected number, found {other:?}"),
            )),
        }
    }
}

// ── Free helpers ──────────────────────────────────────────────────────────────

fn traversal(
    input: EnumSet<Role>,
    kind: EnumSet<WeightKind>,
    output: EnumSet<Role>,
    depth: TraversalDepth,
) -> TraversalSpec {
    TraversalSpec {
        input_roles: input,
        kind_filter: kind,
        output_roles: output,
        depth,
        inverted: false,
    }
}

/// Inject a seed TapeFn onto the first step of a composition branch.
/// If the branch has no steps, creates a seed-only Identity step.
fn inject_seed_into_branch(
    seed: Option<TapeFn>,
    mut steps: Vec<ProjectionStep>,
) -> Vec<ProjectionStep> {
    match seed {
        Some(tap) => {
            if steps.is_empty() {
                vec![ProjectionStep::with_input(tap, StepOperation::Identity)]
            } else {
                steps[0].input = tap;
                steps
            }
        }
        None => steps,
    }
}

fn label_steps(steps: &mut [ProjectionStep]) {
    for (i, step) in steps.iter_mut().enumerate() {
        if step.label.is_empty() {
            step.label = i.to_string();
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Public parse entry point
// ═════════════════════════════════════════════════════════════════════════════

/// Parse a query string into a [`QuerySpec`].
///
/// The query string uses the noet textual grammar defined in
/// `docs/design/query_model.md §9.5`. View configuration (sort, display mode)
/// is **not** part of the query string — it travels as sibling URL parameters,
/// directive options, or MCP fields.
///
/// # Errors
///
/// Returns a [`ParseError`] with a byte offset and human-readable message if
/// the input does not conform to the grammar.
pub fn parse(input: &str) -> Result<QuerySpec, ParseError> {
    let tokens = tokenise(input)?;
    let len = input.len();
    let mut parser = Parser::new(tokens, len);
    let spec = parser.parse_query()?;
    if !matches!(parser.peek_at(0), Token::Eof) {
        return Err(ParseError::new(
            parser.current_offset(),
            format!(
                "unexpected token {:?} after query expression",
                parser.peek_at(0)
            ),
        ));
    }
    Ok(spec)
}

// ═════════════════════════════════════════════════════════════════════════════
// Serializer
// ═════════════════════════════════════════════════════════════════════════════

/// Serialize a [`QuerySpec`] to its canonical textual form.
///
/// The serializer is the inverse of [`parse`]: `parse(serialize(spec)) == spec`
/// for all valid `QuerySpec` values representable in the textual grammar.
/// Named shorthands are re-emitted when the traversal matches a known pattern.
pub fn serialize(spec: &QuerySpec) -> String {
    let mut out = String::new();
    serialize_steps(&mut out, &spec.steps, None);
    out.trim_end().to_string()
}

fn serialize_steps(out: &mut String, steps: &[ProjectionStep], parent_prec: Option<u8>) {
    let mut first = true;
    for step in steps {
        if first {
            // First step: emit seed TapeFn prefix if present
            serialize_seed_prefix(out, &step.input);
        } else {
            match &step.input {
                TapeFn::Then(None) => {
                    let _ = write!(out, " ");
                }
                TapeFn::Then(Some(r)) => {
                    let _ = write!(out, " THEN({}) ", serialize_step_ref(r));
                }
                TapeFn::Fold { op, range } => {
                    let _ = write!(out, " FOLD({}", serialize_set_op(*op));
                    if let Some((a, b)) = range {
                        let _ = write!(out, ",{},{}", serialize_step_ref(a), serialize_step_ref(b));
                    }
                    let _ = write!(out, ") ");
                }
                TapeFn::Terminal(range) => {
                    let _ = write!(out, " TERMINAL");
                    if let Some((a, b)) = range {
                        let _ =
                            write!(out, "({},{})", serialize_step_ref(a), serialize_step_ref(b));
                    }
                    let _ = write!(out, " ");
                }
                TapeFn::Orphan(range) => {
                    let _ = write!(out, " ORPHAN");
                    if let Some((a, b)) = range {
                        let _ =
                            write!(out, "({},{})", serialize_step_ref(a), serialize_step_ref(b));
                    }
                    let _ = write!(out, " ");
                }
                // Mid-pipeline seed: emit as explicit seed function
                TapeFn::Bids(_) | TapeFn::Keys(_) | TapeFn::Corpus | TapeFn::DocumentNodes(..) => {
                    let _ = write!(out, " ");
                    serialize_seed_prefix(out, &step.input);
                }
            }
        }
        serialize_operation(out, &step.operation, parent_prec);
        first = false;
    }
}

/// Emit a seed TapeFn as a grammar prefix. No-op for non-seed variants.
fn serialize_seed_prefix(out: &mut String, tap: &TapeFn) {
    match tap {
        TapeFn::Keys(keys) if keys.len() == 1 => {
            let _ = write!(out, "{} ", keys[0]);
        }
        TapeFn::Keys(keys) if keys.len() > 1 => {
            let _ = write!(out, "KEYS(");
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    let _ = write!(out, ",");
                }
                let _ = write!(out, "{key}");
            }
            let _ = write!(out, ") ");
        }
        TapeFn::Corpus => {
            let _ = write!(out, "CORPUS() ");
        }
        TapeFn::Bids(bids) => {
            let _ = write!(out, "BIDS(");
            for (i, bid) in bids.iter().enumerate() {
                if i > 0 {
                    let _ = write!(out, ",");
                }
                let _ = write!(out, "{bid}");
            }
            let _ = write!(out, ") ");
        }
        _ => {} // Then, Fold, etc. — no prefix for first step
    }
}

fn serialize_operation(out: &mut String, op: &StepOperation, parent_prec: Option<u8>) {
    match op {
        StepOperation::Filter(f) => serialize_filter(out, f),
        StepOperation::Traverse(t) => serialize_traversal(out, t),
        StepOperation::Compose(c) => serialize_composition(out, c, parent_prec),
        StepOperation::Identity => {} // pass-through, nothing to emit
    }
}

fn serialize_filter(out: &mut String, f: &NodeFilter) {
    match f {
        NodeFilter::TextMatch { path, query } => {
            // TextMatch always serialises as path:term.
            // Multi-word terms are quoted; single words are unquoted.
            let path_str = serialize_property_path(path);
            if query.contains(' ') || query.is_empty() {
                let _ = write!(out, "{path_str}:\"{query}\"");
            } else {
                let _ = write!(out, "{path_str}:{query}");
            }
        }
        NodeFilter::Predicate(pp) => {
            let path_str = serialize_property_path(&pp.path);
            match &pp.op {
                CompareOp::Eq => {
                    let _ = write!(out, "{path_str} == {}", serialize_value(&pp.value));
                }
                CompareOp::NotEq => {
                    let _ = write!(out, "{path_str} != {}", serialize_value(&pp.value));
                }
                CompareOp::Gt => {
                    let _ = write!(out, "{path_str} > {}", serialize_value(&pp.value));
                }
                CompareOp::Lt => {
                    let _ = write!(out, "{path_str} < {}", serialize_value(&pp.value));
                }
                CompareOp::Gte => {
                    let _ = write!(out, "{path_str} >= {}", serialize_value(&pp.value));
                }
                CompareOp::Lte => {
                    let _ = write!(out, "{path_str} <= {}", serialize_value(&pp.value));
                }
                CompareOp::In => {
                    if let PropertyValue::Set(items) = &pp.value {
                        let _ = write!(out, "{path_str} in ({})", items.join(","));
                    }
                }
                CompareOp::Matches => {
                    let _ = write!(out, "{path_str} matches {}", serialize_value(&pp.value));
                }
                CompareOp::Contains => {
                    let _ = write!(out, "{path_str} contains {}", serialize_value(&pp.value));
                }
                CompareOp::Exists => {
                    let _ = write!(out, "{path_str} exists");
                }
            }
        }
    }
}

fn serialize_traversal(out: &mut String, t: &TraversalSpec) {
    if t.inverted {
        let _ = write!(out, "!");
    }
    // Try named shorthands first (derived from DIRECTIVES verb names and TraversalSpec).
    // All single-role patterns with no edge filter can be expressed as a named verb.
    if t.depth.edge_filter.is_none() {
        macro_rules! emit_shorthand {
            ($input:expr, $kind:expr, $output:expr, $name:literal) => {
                if t.input_roles == $input && t.kind_filter == $kind && t.output_roles == $output {
                    let _ = write!(
                        out,
                        concat!($name, "({})"),
                        serialize_depth_count(t.depth.count)
                    );
                    return;
                }
            };
        }
        emit_shorthand!(Role::Sink, WeightKind::Section, Role::Source, "composed_of");
        emit_shorthand!(
            Role::Source,
            WeightKind::Section,
            Role::Sink,
            "component_of"
        );
        emit_shorthand!(Role::Sink, WeightKind::Pragmatic, Role::Source, "uses");
        emit_shorthand!(Role::Source, WeightKind::Pragmatic, Role::Sink, "used_by");
        emit_shorthand!(
            Role::Sink,
            WeightKind::Epistemic,
            Role::Source,
            "draws_from"
        );
        emit_shorthand!(Role::Source, WeightKind::Epistemic, Role::Sink, "underlies");
        emit_shorthand!(
            Role::Owner,
            WeightKind::Pragmatic,
            Role::Source | Role::Sink,
            "covers"
        );
    }

    // Full form
    let _ = write!(
        out,
        "{}-{}-{}",
        serialize_roles(t.input_roles),
        serialize_kinds(t.kind_filter),
        serialize_roles(t.output_roles),
    );
    if t.depth.count != DepthCount::N(1) || t.depth.edge_filter.is_some() {
        let _ = write!(out, "(");
        let _ = write!(out, "{}", serialize_depth_count(t.depth.count));
        if let Some(ref ef) = t.depth.edge_filter {
            let _ = write!(
                out,
                ",{}:{}",
                serialize_property_path(&ef.path),
                serialize_value(&ef.value),
            );
        }
        let _ = write!(out, ")");
    }
    let _ = out; // suppress unused warning
}

/// Precedence of composition operators for minimal parenthesization.
/// Higher number = tighter binding.
fn comp_precedence(op: CompositionOp) -> u8 {
    match op {
        CompositionOp::Or => 0,
        // AND and NOT (binary infix) share the same precedence level
        CompositionOp::And | CompositionOp::Not => 1,
    }
}

fn serialize_composition(out: &mut String, c: &Composition, parent_prec: Option<u8>) {
    let op_str = match c.op {
        CompositionOp::And => "AND",
        CompositionOp::Or => "OR",
        CompositionOp::Not => "NOT",
    };

    // Detect unary NOT (pass_all on left) — filter-level NOT
    let is_filter_not = c.op == CompositionOp::Not && c.left.len() == 1 && is_pass_all(&c.left[0]);

    if is_filter_not {
        let _ = write!(out, "NOT ");
        if c.right.len() == 1 {
            serialize_operation(out, &c.right[0].operation, None);
        } else {
            let _ = write!(out, "(");
            serialize_steps(out, &c.right, None);
            let _ = write!(out, ")");
        }
        return;
    }

    let my_prec = comp_precedence(c.op);
    // Emit parens when this composition has lower precedence than its parent
    let needs_parens = parent_prec.is_some_and(|pp| my_prec < pp);

    if needs_parens {
        let _ = write!(out, "(");
    }
    serialize_steps(out, &c.left, Some(my_prec));
    let _ = write!(out, " {op_str} ");
    serialize_steps(out, &c.right, Some(my_prec));
    if needs_parens {
        let _ = write!(out, ")");
    }
}

fn is_pass_all(step: &ProjectionStep) -> bool {
    if let StepOperation::Filter(NodeFilter::Predicate(pp)) = &step.operation {
        pp.op == CompareOp::Exists
            && matches!(pp.value, PropertyValue::None)
            && path_to_field_name(&pp.path) == Some("kind")
    } else {
        false
    }
}

// ── Serializer helpers ────────────────────────────────────────────────────────

fn serialize_roles(roles: EnumSet<Role>) -> String {
    if roles == (Role::Source | Role::Sink | Role::Owner) {
        return "n".to_string();
    }
    let mut s = String::new();
    if roles.contains(Role::Source) {
        s.push('s');
    }
    if roles.contains(Role::Sink) {
        s.push('k');
    }
    if roles.contains(Role::Owner) {
        s.push('o');
    }
    s
}

fn serialize_kinds(kinds: EnumSet<WeightKind>) -> String {
    if kinds == EnumSet::all() {
        return "*".to_string();
    }
    let parts: Vec<&str> = [
        WeightKind::Section,
        WeightKind::Epistemic,
        WeightKind::Pragmatic,
    ]
    .iter()
    .filter(|&&k| kinds.contains(k))
    .map(|&k| kind_name(k))
    .collect();
    parts.join(",")
}

fn serialize_depth_count(c: DepthCount) -> String {
    match c {
        DepthCount::N(n) => n.to_string(),
        DepthCount::Max => "*".to_string(),
    }
}

fn serialize_property_path(path: &PropertyPath) -> String {
    path.iter()
        .map(|seg| match seg {
            PropertySegment::Key(k) => k.clone(),
            PropertySegment::Index(i) => format!("[{i}]"),
            PropertySegment::Slice(a, b) => format!("[{a}..{b}]"),
            PropertySegment::Wildcard => "*".to_string(),
            PropertySegment::GlobStar => "**".to_string(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn path_to_field_name(path: &PropertyPath) -> Option<&str> {
    if path.len() == 1 {
        if let PropertySegment::Key(name) = &path[0] {
            return Some(name.as_str());
        }
    }
    None
}

fn serialize_value(v: &PropertyValue) -> String {
    match v {
        PropertyValue::String(s) => {
            if s.contains(' ') || s.is_empty() {
                format!("\"{s}\"")
            } else {
                s.clone()
            }
        }
        PropertyValue::Number(n) => {
            if n.fract() == 0.0 {
                format!("{}", *n as i64)
            } else {
                format!("{n}")
            }
        }
        PropertyValue::Set(items) => format!("({})", items.join(",")),
        PropertyValue::Regex(r) => format!("\"{r}\""),
        PropertyValue::None => String::new(),
    }
}

fn serialize_set_op(op: SetOp) -> &'static str {
    match op {
        SetOp::Union => "UNION",
        SetOp::Intersection => "INTERSECT",
        SetOp::LeftDiff => "LDIFF",
        SetOp::RightDiff => "RDIFF",
        SetOp::SymmetricDiff => "SYMDIFF",
    }
}

fn serialize_step_ref(r: &StepRef) -> String {
    match r {
        StepRef::Label(l) => l.clone(),
        StepRef::Index(i) => i.to_string(),
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Tests
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(input: &str) -> QuerySpec {
        parse(input).unwrap_or_else(|e| panic!("parse failed for '{input}': {e}"))
    }

    fn first_traverse(spec: &QuerySpec) -> &TraversalSpec {
        match &spec.steps[0].operation {
            StepOperation::Traverse(t) => t,
            other => panic!("expected Traverse, got {other:?}"),
        }
    }

    fn first_filter(spec: &QuerySpec) -> &NodeFilter {
        match &spec.steps[0].operation {
            StepOperation::Filter(f) => f,
            other => panic!("expected Filter, got {other:?}"),
        }
    }

    #[test]
    fn simple_traversal() {
        let spec = parse_ok("s-pragmatic-k");
        let t = first_traverse(&spec);
        assert_eq!(t.input_roles, Role::Source);
        assert_eq!(t.kind_filter, WeightKind::Pragmatic);
        assert_eq!(t.output_roles, Role::Sink);
        assert_eq!(t.depth.count, DepthCount::N(1));
    }

    #[test]
    fn traversal_with_depth() {
        let spec = parse_ok("s-pragmatic-k(3)");
        assert_eq!(first_traverse(&spec).depth.count, DepthCount::N(3));
    }

    #[test]
    fn unbounded_depth() {
        let spec = parse_ok("s-section-k(*)");
        assert_eq!(first_traverse(&spec).depth.count, DepthCount::Max);
    }

    #[test]
    fn multi_role_traversal() {
        let spec = parse_ok("sk-pragmatic-o");
        let t = first_traverse(&spec);
        assert_eq!(t.input_roles, Role::Source | Role::Sink);
        assert_eq!(t.output_roles, Role::Owner);
    }

    #[test]
    fn n_wildcard_roles() {
        let spec = parse_ok("n-pragmatic-n");
        let t = first_traverse(&spec);
        assert_eq!(t.input_roles, Role::Source | Role::Sink | Role::Owner);
        assert_eq!(t.output_roles, Role::Source | Role::Sink | Role::Owner);
    }

    #[test]
    fn shorthand_composed_of() {
        // composed_of(N) = k-section-s(N): root→leaf (submap direction)
        let a = parse_ok("composed_of(3)");
        let b = parse_ok("k-section-s(3)");
        assert_eq!(first_traverse(&a), first_traverse(&b));
    }

    #[test]
    fn shorthand_consists_of_alias() {
        // consists_of is a backward-compatible alias for composed_of
        let a = parse_ok("consists_of(3)");
        let b = parse_ok("composed_of(3)");
        assert_eq!(first_traverse(&a), first_traverse(&b));
    }

    #[test]
    fn shorthand_component_of() {
        // component_of(N) = s-section-k(N): leaf→root
        let a = parse_ok("component_of(3)");
        let b = parse_ok("s-section-k(3)");
        assert_eq!(first_traverse(&a), first_traverse(&b));
    }

    #[test]
    fn shorthand_uses() {
        // uses(N) = k-pragmatic-s(N)
        let a = parse_ok("uses(1)");
        let b = parse_ok("k-pragmatic-s(1)");
        assert_eq!(first_traverse(&a), first_traverse(&b));
    }

    #[test]
    fn shorthand_implements_synonym() {
        // implements is a synonym for uses
        let a = parse_ok("implements(1)");
        let b = parse_ok("uses(1)");
        assert_eq!(first_traverse(&a), first_traverse(&b));
    }

    #[test]
    fn shorthand_used_by() {
        // used_by(N) = s-pragmatic-k(N)
        let a = parse_ok("used_by(1)");
        let b = parse_ok("s-pragmatic-k(1)");
        assert_eq!(first_traverse(&a), first_traverse(&b));
    }

    #[test]
    fn shorthand_covers() {
        // covers(N) = o-epistemic-sk(N) (maps_to default is epistemic)
        let a = parse_ok("covers(2)");
        let b = parse_ok("o-epistemic-sk(2)");
        assert_eq!(first_traverse(&a), first_traverse(&b));
    }

    #[test]
    fn shorthand_constrained_by() {
        let a = parse_ok("constrained_by(1)");
        let b = parse_ok("k-epistemic-s(1)");
        assert_eq!(first_traverse(&a), first_traverse(&b));
    }

    #[test]
    fn shorthand_constrains() {
        let a = parse_ok("constrains(1)");
        let b = parse_ok("s-epistemic-k(1)");
        assert_eq!(first_traverse(&a), first_traverse(&b));
    }

    #[test]
    fn shorthand_constrained_by_equals_draws_from() {
        let a = parse_ok("constrained_by(2)");
        let b = parse_ok("draws_from(2)");
        assert_eq!(first_traverse(&a), first_traverse(&b));
    }

    #[test]
    fn shorthand_constrains_equals_underlies() {
        let a = parse_ok("constrains(2)");
        let b = parse_ok("underlies(2)");
        assert_eq!(first_traverse(&a), first_traverse(&b));
    }

    #[test]
    fn shorthand_draws_from() {
        let a = parse_ok("draws_from(1)");
        let b = parse_ok("k-epistemic-s(1)");
        assert_eq!(first_traverse(&a), first_traverse(&b));
    }

    #[test]
    fn shorthand_underlies() {
        let a = parse_ok("underlies(1)");
        let b = parse_ok("s-epistemic-k(1)");
        assert_eq!(first_traverse(&a), first_traverse(&b));
    }

    #[test]
    fn arrow_syntax_produces_helpful_error() {
        let err = parse("->pragmatic(1)").unwrap_err();
        assert!(err.message.contains("named shorthands"), "{}", err.message);
    }

    #[test]
    fn anchor_sets_subject_keys() {
        let spec = parse_ok("id://priority-high k-pragmatic-s(1)");
        // First step has the seed TapeFn::Keys, second step has the traversal
        // (seed_then puts Keys on the first step's input)
        assert!(matches!(&spec.steps[0].input, TapeFn::Keys(_)));
    }

    #[test]
    fn bare_anchor_produces_seed_only() {
        let spec = parse_ok("bref:abc123def456");
        assert_eq!(spec.steps.len(), 1);
        assert!(matches!(&spec.steps[0].input, TapeFn::Keys(_)));
        assert!(matches!(&spec.steps[0].operation, StepOperation::Identity));
    }

    #[test]
    fn bare_corpus_produces_seed_only() {
        let spec = parse_ok("CORPUS()");
        assert_eq!(spec.steps.len(), 1);
        assert!(matches!(&spec.steps[0].input, TapeFn::Corpus));
        assert!(matches!(&spec.steps[0].operation, StepOperation::Identity));
    }

    #[test]
    fn bid_anchor_round_trips() {
        // A bid: anchor must serialize and re-parse correctly.
        let bid_str = "bid:01930f88-7f1a-6df0-8ec3-3f277ca48864";
        let input = format!("{bid_str} composed_of(1)");
        let spec = parse_ok(&input);
        let serialized = serialize(&spec);
        assert!(
            serialized.starts_with("bid:"),
            "expected bid: prefix, got: {serialized}"
        );
        let spec2 = parse(&serialized)
            .unwrap_or_else(|e| panic!("round-trip failed: '{input}' → '{serialized}': {e}"));
        assert_eq!(spec.steps[0].input, spec2.steps[0].input);
    }

    #[test]
    fn bare_word_is_parse_error() {
        // Bare words without a field prefix are no longer valid.
        // TextMatch always requires explicit field:term syntax.
        assert!(parse("authentication").is_err());
        assert!(parse("foo").is_err());
    }

    #[test]
    fn explicit_text_match_single_word() {
        let spec = parse_ok("title:auth");
        match first_filter(&spec) {
            NodeFilter::TextMatch { path, query } => {
                assert_eq!(path_to_field_name(path), Some("title"));
                assert_eq!(query, "auth");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn explicit_text_match_multi_word_requires_quotes() {
        // Multi-word terms must be quoted
        let spec = parse_ok("title:\"auth flow\"");
        match first_filter(&spec) {
            NodeFilter::TextMatch { path, query } => {
                assert_eq!(path_to_field_name(path), Some("title"));
                assert_eq!(query, "auth flow");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn text_field_is_multi_field_search() {
        // text: is the all-indexed-fields TextMatch
        let spec = parse_ok("text:authentication");
        match first_filter(&spec) {
            NodeFilter::TextMatch { path, query } => {
                assert_eq!(path_to_field_name(path), Some("text"));
                assert_eq!(query, "authentication");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bare_colon_expands_to_text_field() {
        // :term is shorthand for text:term — the colon alone is the TextMatch sigil.
        // No lexer change needed: ':' was already emitted as Word(":").
        let a = parse_ok(":authentication");
        let b = parse_ok("text:authentication");
        assert_eq!(a.steps, b.steps);

        // Multi-word with quotes: :"auth flow" expands to text:"auth flow"
        let c = parse_ok(":auth_flow"); // single unquoted word
        match first_filter(&c) {
            NodeFilter::TextMatch { path, query } => {
                assert_eq!(path_to_field_name(path), Some("text"));
                assert_eq!(query, "auth_flow");
            }
            other => panic!("{other:?}"),
        }
        // Quoted multi-word
        let d = parse_ok(r#":"auth flow""#);
        match first_filter(&d) {
            NodeFilter::TextMatch { path, query } => {
                assert_eq!(path_to_field_name(path), Some("text"));
                assert_eq!(query, "auth flow");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn any_field_prefix_is_text_match() {
        // Any word:term is a TextMatch — no special-casing for known fields
        let spec = parse_ok("foo:bar");
        match first_filter(&spec) {
            NodeFilter::TextMatch { path, query } => {
                assert_eq!(path_to_field_name(path), Some("foo"));
                assert_eq!(query, "bar");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn predicate_eq() {
        let spec = parse_ok("schema == procedure");
        match first_filter(&spec) {
            NodeFilter::Predicate(pp) => {
                assert_eq!(pp.op, CompareOp::Eq);
                assert_eq!(pp.value, PropertyValue::String("procedure".into()));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn predicate_eq_quoted() {
        let spec = parse_ok("schema == \"procedure\"");
        match first_filter(&spec) {
            NodeFilter::Predicate(pp) => {
                assert_eq!(pp.value, PropertyValue::String("procedure".into()));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn predicate_exists() {
        let spec = parse_ok("metadata.git.branch exists");
        match first_filter(&spec) {
            NodeFilter::Predicate(pp) => {
                assert_eq!(pp.op, CompareOp::Exists);
                assert!(matches!(pp.value, PropertyValue::None));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn predicate_gt() {
        let spec = parse_ok("payload.priority > 3");
        match first_filter(&spec) {
            NodeFilter::Predicate(pp) => {
                assert_eq!(pp.op, CompareOp::Gt);
                assert_eq!(pp.value, PropertyValue::Number(3.0));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn predicate_in_set() {
        let spec = parse_ok("kind in (Document,Symbol)");
        match first_filter(&spec) {
            NodeFilter::Predicate(pp) => {
                assert_eq!(pp.op, CompareOp::In);
                assert_eq!(
                    pp.value,
                    PropertyValue::Set(vec!["Document".into(), "Symbol".into()])
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn filter_and_produces_composition() {
        let spec = parse_ok("title:auth AND schema:procedure");
        match &spec.steps[0].operation {
            StepOperation::Compose(c) => assert_eq!(c.op, CompositionOp::And),
            other => panic!("expected Compose, got {other:?}"),
        }
    }

    #[test]
    fn pipeline_two_stages() {
        let spec = parse_ok("s-section-k(3) s-pragmatic-k(1)");
        assert_eq!(spec.steps.len(), 2);
    }

    #[test]
    fn pipeline_with_then_keyword() {
        let a = parse_ok("s-section-k(3) THEN s-pragmatic-k(1)");
        let b = parse_ok("s-section-k(3) s-pragmatic-k(1)");
        assert_eq!(a.steps.len(), b.steps.len());
    }

    #[test]
    fn query_level_and() {
        let spec = parse_ok("k-pragmatic-s(1) AND o-pragmatic-k(2)");
        assert_eq!(spec.steps.len(), 1);
        assert!(matches!(
            &spec.steps[0].operation,
            StepOperation::Compose(_)
        ));
    }

    #[test]
    fn degenerate_traversal_rejected() {
        assert!(parse("s-pragmatic-s").is_err());
        assert!(parse("k-section-k").is_err());
    }

    #[test]
    fn lowercase_and_is_operator() {
        // Keywords are case-insensitive: "and" normalises to "AND" in the lexer.
        // Bare "foo and bar" is now a parse error because bare words require a
        // field prefix. But field:term And field:term works in any case.
        assert!(parse("foo and bar").is_err());

        // Mixed case works when terms have explicit field prefixes
        let spec = parse_ok("title:auth And schema:procedure");
        match &spec.steps[0].operation {
            StepOperation::Compose(c) => assert_eq!(c.op, CompositionOp::And),
            other => panic!("expected Compose(And), got {other:?}"),
        }

        // text:foo AND text:bar works (explicit field on both sides)
        let spec2 = parse_ok("text:foo and text:bar");
        match &spec2.steps[0].operation {
            StepOperation::Compose(c) => assert_eq!(c.op, CompositionOp::And),
            other => panic!("expected Compose(And), got {other:?}"),
        }
    }

    #[test]
    fn any_field_colon_is_text_match_on_that_field() {
        // Previously unknown fields fell back to content search.
        // Now any word:term is a TextMatch on that property path.
        let spec = parse_ok("foo:bar");
        match first_filter(&spec) {
            NodeFilter::TextMatch { path, query } => {
                assert_eq!(path_to_field_name(path), Some("foo"));
                assert_eq!(query, "bar");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn roots_expansion() {
        let spec = parse_ok("roots()");
        assert_eq!(spec.steps.len(), 2);
        // Also accept (*) form
        let spec2 = parse_ok("roots(*)");
        assert_eq!(spec2.steps.len(), 2);
    }

    #[test]
    fn leaves_expansion() {
        let spec = parse_ok("leaves()");
        assert_eq!(spec.steps.len(), 2);
    }

    #[test]
    fn halo_expansion() {
        let spec = parse_ok("halo()");
        assert_eq!(spec.steps.len(), 1);
        let t = first_traverse(&spec);
        assert_eq!(t.input_roles, Role::Source | Role::Sink | Role::Owner);
    }

    #[test]
    fn terminal_tape_fn() {
        let spec = parse_ok("s-section-k(*) TERMINAL kind exists");
        assert_eq!(spec.steps.len(), 2);
        assert!(matches!(spec.steps[1].input, TapeFn::Terminal(None)));
    }

    #[test]
    fn serialize_simple_traversal() {
        let spec = parse_ok("s-pragmatic-k(1)");
        let s = serialize(&spec);
        // Should round-trip
        let spec2 = parse(&s).unwrap();
        assert_eq!(first_traverse(&spec), first_traverse(&spec2));
    }

    #[test]
    fn serialize_shorthand() {
        // used_by(1) = s-pragmatic-k(1) — serialises back to named form
        let spec = parse_ok("used_by(1)");
        let s = serialize(&spec);
        assert!(s.contains("used_by"), "expected named shorthand, got: {s}");

        // uses(1) = k-pragmatic-s(1)
        let spec2 = parse_ok("uses(1)");
        let s2 = serialize(&spec2);
        assert!(s2.contains("uses"), "expected named shorthand, got: {s2}");
    }

    #[test]
    fn keys_single_parses_like_bare_anchor() {
        let bare = parse_ok("bref:abc123def456 composed_of(1)");
        let keys = parse_ok("KEYS(bref:abc123def456) composed_of(1)");
        assert_eq!(bare.steps.len(), keys.steps.len());
        assert_eq!(bare.steps[0].input, keys.steps[0].input);
    }

    #[test]
    fn keys_multi_parses_multiple_keys() {
        let spec = parse_ok("KEYS(bref:abc123def456,bref:fedcba654321) composed_of(1)");
        assert_eq!(spec.steps.len(), 1);
        match &spec.steps[0].input {
            TapeFn::Keys(keys) => assert_eq!(keys.len(), 2),
            other => panic!("expected TapeFn::Keys, got {other:?}"),
        }
    }

    #[test]
    fn corpus_parses() {
        let spec = parse_ok("CORPUS() :authentication");
        assert_eq!(spec.steps.len(), 1);
        assert!(matches!(spec.steps[0].input, TapeFn::Corpus));
    }

    #[test]
    fn corpus_case_insensitive() {
        let spec = parse_ok("corpus() :auth");
        assert!(matches!(spec.steps[0].input, TapeFn::Corpus));
    }

    #[test]
    fn keys_empty_is_error() {
        assert!(parse("KEYS() composed_of(1)").is_err());
    }

    #[test]
    fn keys_multi_round_trip() {
        let input = "KEYS(bref:abc123def456,bref:fedcba654321) composed_of(1)";
        let spec = parse_ok(input);
        let serialized = serialize(&spec);
        assert!(
            serialized.starts_with("KEYS("),
            "multi-key should serialize as KEYS(...), got: {serialized}"
        );
        let spec2 = parse(&serialized)
            .unwrap_or_else(|e| panic!("round-trip failed: '{input}' → '{serialized}': {e}"));
        assert_eq!(spec.steps.len(), spec2.steps.len());
        assert_eq!(spec.steps[0].input, spec2.steps[0].input);
    }

    #[test]
    fn corpus_round_trip() {
        let input = "CORPUS() :authentication";
        let spec = parse_ok(input);
        let serialized = serialize(&spec);
        assert!(
            serialized.starts_with("CORPUS()"),
            "corpus should serialize as CORPUS(), got: {serialized}"
        );
        let spec2 = parse(&serialized)
            .unwrap_or_else(|e| panic!("round-trip failed: '{input}' → '{serialized}': {e}"));
        assert_eq!(spec.steps[0].input, spec2.steps[0].input);
    }

    #[test]
    fn single_key_serializes_as_bare_anchor() {
        let input = "bref:abc123def456 composed_of(1)";
        let spec = parse_ok(input);
        let serialized = serialize(&spec);
        assert!(
            !serialized.starts_with("KEYS("),
            "single key should serialize as bare anchor, got: {serialized}"
        );
        assert!(
            serialized.starts_with("bref:abc123def456"),
            "expected bare bref prefix, got: {serialized}"
        );
    }

    #[test]
    fn composition_with_seeds() {
        let spec =
            parse_ok("KEYS(bref:abc123def456) composed_of(1) AND KEYS(bref:fedcba654321) uses(1)");
        assert_eq!(spec.steps.len(), 1);
        match &spec.steps[0].operation {
            StepOperation::Compose(c) => {
                assert_eq!(c.left.len(), 1);
                assert_eq!(c.right.len(), 1);
                assert!(matches!(c.left[0].input, TapeFn::Keys(_)));
                assert!(matches!(c.right[0].input, TapeFn::Keys(_)));
            }
            other => panic!("expected Compose, got {other:?}"),
        }
    }

    #[test]
    fn composition_with_bare_anchors() {
        // Bare bid: anchors on both sides of AND must parse as two
        // independently-seeded pipelines composed together.
        let spec = parse_ok(
            "bid:1f15a08c-8116-626e-aa0a-57890d6a7644 uses(1) \
             AND bid:1f15a08c-8116-676e-aa0c-57890d6a7644 uses(1)",
        );
        assert_eq!(spec.steps.len(), 1);
        match &spec.steps[0].operation {
            StepOperation::Compose(c) => {
                assert_eq!(c.op, CompositionOp::And);
                assert_eq!(c.left.len(), 1);
                assert_eq!(c.right.len(), 1);
                assert!(matches!(c.left[0].input, TapeFn::Keys(_)));
                assert!(matches!(c.right[0].input, TapeFn::Keys(_)));
            }
            other => panic!("expected Compose, got {other:?}"),
        }
    }

    #[test]
    fn round_trip_predicate() {
        let inputs = [
            "schema == procedure",
            "payload.priority > 3",
            "metadata.git.branch exists",
            "kind in (Document,Symbol)",
            "title:auth",
            "text:authentication",
            "title:\"auth flow\"",
            "s-section-k(*)",
            "covers(2)",
            "composed_of(3)",
            "component_of(1)",
            "uses(1)",
            "used_by(1)",
        ];
        for input in inputs {
            let spec = parse_ok(input);
            let serialized = serialize(&spec);
            let spec2 = parse(&serialized).unwrap_or_else(|e| {
                panic!("round-trip failed for '{input}' → '{serialized}': {e}")
            });
            assert_eq!(
                spec.steps.len(),
                spec2.steps.len(),
                "step count mismatch for '{input}'"
            );
        }
    }

    // ── Composition precedence and grouping tests ─────────────────────

    /// Helper: extract the Composition from a single-step spec.
    fn extract_compose(spec: &QuerySpec) -> &Composition {
        match &spec.steps[0].operation {
            StepOperation::Compose(c) => c,
            other => panic!("expected Compose, got {other:?}"),
        }
    }

    /// Helper: extract the CompositionOp from the outermost Composition.
    fn outer_op(spec: &QuerySpec) -> CompositionOp {
        extract_compose(spec).op
    }

    /// Helper: check that the left branch of the outermost Composition
    /// is itself a Composition with the given operator.
    fn left_inner_op(spec: &QuerySpec) -> CompositionOp {
        let c = extract_compose(spec);
        match &c.left[0].operation {
            StepOperation::Compose(inner) => inner.op,
            other => panic!("expected inner Compose on left, got {other:?}"),
        }
    }

    /// Helper: check that the right branch of the outermost Composition
    /// is itself a Composition with the given operator.
    fn right_inner_op(spec: &QuerySpec) -> CompositionOp {
        let c = extract_compose(spec);
        match &c.right[0].operation {
            StepOperation::Compose(inner) => inner.op,
            other => panic!("expected inner Compose on right, got {other:?}"),
        }
    }

    #[test]
    fn precedence_and_binds_tighter_than_or() {
        // A OR B AND C should parse as A OR (B AND C)
        // Using anchored pipelines so composition-level precedence applies
        let spec = parse_ok(
            "id://a k-pragmatic-s(1) OR id://b k-pragmatic-s(1) AND id://c k-pragmatic-s(1)",
        );
        assert_eq!(outer_op(&spec), CompositionOp::Or);
        assert_eq!(right_inner_op(&spec), CompositionOp::And);
    }

    #[test]
    fn precedence_and_or_chain() {
        // A AND B OR C should parse as (A AND B) OR C
        let spec = parse_ok(
            "id://a k-pragmatic-s(1) AND id://b k-pragmatic-s(1) OR id://c k-pragmatic-s(1)",
        );
        assert_eq!(outer_op(&spec), CompositionOp::Or);
        assert_eq!(left_inner_op(&spec), CompositionOp::And);
    }

    #[test]
    fn precedence_not_binary_same_as_and() {
        // For bare filter terms, AND/OR/NOT are consumed by the filter-level
        // parser (same precedence rules as filter expressions). Composition-
        // level precedence applies between multi-stage or anchored pipelines.
        //
        // id://a trav NOT id://b trav OR id://c trav
        // should parse as (id://a trav NOT id://b trav) OR (id://c trav)
        let spec = parse_ok(
            "id://a k-pragmatic-s(1) NOT id://b k-pragmatic-s(1) OR id://c k-pragmatic-s(1)",
        );
        assert_eq!(outer_op(&spec), CompositionOp::Or);
        assert_eq!(left_inner_op(&spec), CompositionOp::Not);
    }

    #[test]
    fn parenthesized_grouping_overrides_precedence() {
        // A AND (B OR C) — parens force OR to bind before AND at composition level
        let spec = parse_ok(
            "id://a k-pragmatic-s(1) AND (id://b k-pragmatic-s(1) OR id://c k-pragmatic-s(1))",
        );
        assert_eq!(outer_op(&spec), CompositionOp::And);
        assert_eq!(right_inner_op(&spec), CompositionOp::Or);
    }

    #[test]
    fn parenthesized_grouping_left_side() {
        // (A OR B) AND C
        let spec = parse_ok(
            "(id://a k-pragmatic-s(1) OR id://b k-pragmatic-s(1)) AND id://c k-pragmatic-s(1)",
        );
        assert_eq!(outer_op(&spec), CompositionOp::And);
        assert_eq!(left_inner_op(&spec), CompositionOp::Or);
    }

    #[test]
    fn parenthesized_grouping_nested() {
        // (A AND B) OR (C NOT D)
        let spec = parse_ok(
            "(id://a k-pragmatic-s(1) AND id://b k-pragmatic-s(1)) OR (id://c k-pragmatic-s(1) NOT id://d k-pragmatic-s(1))",
        );
        assert_eq!(outer_op(&spec), CompositionOp::Or);
        assert_eq!(left_inner_op(&spec), CompositionOp::And);
        assert_eq!(right_inner_op(&spec), CompositionOp::Not);
    }

    #[test]
    fn unary_not_at_query_level() {
        // NOT pipeline should parse as pass_all NOT pipeline
        let spec = parse_ok("NOT id://x k-pragmatic-s(1)");
        let c = extract_compose(&spec);
        assert_eq!(c.op, CompositionOp::Not);
        assert!(is_pass_all(&c.left[0]));
    }

    #[test]
    fn inverted_traversal_parse_and_round_trip() {
        // !uses(1) parses as an inverted traversal
        let spec = parse_ok("id:a composed_of(*) !uses(1)");
        assert!(spec.steps.len() >= 2);
        let last = spec.steps.last().unwrap();
        match &last.operation {
            StepOperation::Traverse(t) => assert!(t.inverted, "should be inverted"),
            other => panic!("expected Traverse, got {other:?}"),
        }

        // !k-pragmatic-s(1) also works (full traversal syntax)
        let spec2 = parse_ok("id:a composed_of(*) !k-pragmatic-s(1)");
        match &spec2.steps.last().unwrap().operation {
            StepOperation::Traverse(t) => assert!(t.inverted),
            other => panic!("expected Traverse, got {other:?}"),
        }

        // Inverse: nodes WITH edges = set NOT !uses(1)
        let spec3 = parse_ok("id:a composed_of(*) NOT (id:a composed_of(*) !uses(1))");
        assert_eq!(outer_op(&spec3), CompositionOp::Not);

        // Round-trip all
        for s in [&spec, &spec2, &spec3] {
            let serialized = serialize(s);
            let rt = serialize(&parse(&serialized).unwrap());
            assert_eq!(serialized, rt, "round-trip: '{serialized}' vs '{rt}'");
        }
    }

    #[test]
    fn terminal_fold_no_following_stage() {
        // FOLD(UNION) at end of pipeline produces Identity step
        let spec = parse_ok("id:a composed_of(*) FOLD(UNION)");
        assert!(spec.steps.len() >= 2);
        let last = spec.steps.last().unwrap();
        assert!(matches!(last.operation, StepOperation::Identity));
        assert!(matches!(last.input, TapeFn::Fold { .. }));
    }

    #[test]
    fn terminal_fold_round_trip() {
        let input = "id:a composed_of(*) FOLD(UNION)";
        let spec = parse_ok(input);
        let serialized = serialize(&spec);
        let spec2 = parse(&serialized)
            .unwrap_or_else(|e| panic!("round-trip failed: '{input}' → '{serialized}': {e}"));
        let serialized2 = serialize(&spec2);
        assert_eq!(
            serialized, serialized2,
            "double round-trip: '{serialized}' vs '{serialized2}'"
        );
    }

    #[test]
    fn chained_tape_fn() {
        // FOLD(UNION) THEN used_by(1) — fold then continue
        let spec = parse_ok("id:a composed_of(*) FOLD(UNION) THEN used_by(1)");
        assert!(spec.steps.len() >= 3);
        // Middle step is Identity with Fold input
        let fold_step = &spec.steps[spec.steps.len() - 2];
        assert!(matches!(fold_step.operation, StepOperation::Identity));
        assert!(matches!(fold_step.input, TapeFn::Fold { .. }));
        // Last step is the traversal
        assert!(matches!(
            spec.steps.last().unwrap().operation,
            StepOperation::Traverse(_)
        ));
    }

    #[test]
    fn terminal_fold_in_composition_arm() {
        // Right arm uses terminal FOLD(UNION) inside parens
        let spec = parse_ok("id:a uses(1) AND (KEYS(id:b,id:c) composed_of(*) FOLD(UNION))");
        assert_eq!(outer_op(&spec), CompositionOp::And);
    }

    #[test]
    fn grouped_composition_with_keyed_rhs() {
        // Exact user query that was failing
        let spec = parse_ok(
            "((id:class-a uses(1) NOT id:class-c uses(1)) THEN used_by(1)) AND (KEYS(id:foo,id:bar) composed_of(*))"
        );
        assert_eq!(outer_op(&spec), CompositionOp::And);
    }

    #[test]
    fn grouped_composition_with_continuation() {
        // Pipelines are greedy: (A NOT B THEN C) = A NOT (B THEN C), not (A NOT B) THEN C.
        // To get post-composition continuation, use inner parens:
        // ((A NOT B) THEN C) = compose A NOT B, then pipe through C.
        let spec = parse_ok("((id:a uses(1) NOT id:c uses(1)) THEN used_by(1)) AND id:d uses(1)");
        assert_eq!(outer_op(&spec), CompositionOp::And);
        // Left branch: the group contains a composition followed by used_by(1)
        let c = extract_compose(&spec);
        assert!(
            c.left.len() > 1,
            "left branch should include composition + continuation stage"
        );
    }

    #[test]
    fn repeated_comp_op_is_error() {
        // Duplicate operators are not silently consumed
        assert!(parse("id:a OR OR id:b").is_err());
        assert!(parse("id:a AND AND id:b").is_err());
    }

    #[test]
    fn chained_unary_not() {
        // NOT NOT title:x = NOT (NOT title:x) = pass_all NOT (pass_all NOT title:x)
        let spec = parse_ok("NOT NOT title:x");
        let c = extract_compose(&spec);
        assert_eq!(c.op, CompositionOp::Not);
        assert!(is_pass_all(&c.left[0]));
        // Right branch is itself a NOT composition
        match &c.right[0].operation {
            StepOperation::Compose(inner) => {
                assert_eq!(inner.op, CompositionOp::Not);
                assert!(is_pass_all(&inner.left[0]));
            }
            other => panic!("expected inner Compose(Not), got {other:?}"),
        }

        // Triple NOT should also work
        let spec3 = parse_ok("NOT NOT NOT title:x");
        let c3 = extract_compose(&spec3);
        assert_eq!(c3.op, CompositionOp::Not);
    }

    #[test]
    fn left_associativity_same_precedence() {
        // A AND B AND C should parse as (A AND B) AND C
        let spec = parse_ok(
            "id://a k-pragmatic-s(1) AND id://b k-pragmatic-s(1) AND id://c k-pragmatic-s(1)",
        );
        assert_eq!(outer_op(&spec), CompositionOp::And);
        assert_eq!(left_inner_op(&spec), CompositionOp::And);
    }

    #[test]
    fn round_trip_composition() {
        // Filter-level compositions (consumed by filter parser)
        let filter_inputs = [
            "title:a AND title:b",
            "title:a OR title:b",
            "title:a NOT title:b",
            "NOT title:x",
        ];
        // Composition-level (anchored pipelines) — use id:X (non-hierarchical)
        // since id://X serializes to id:X (pre-existing NodeKey Display behavior).
        let comp_inputs = [
            "id:a k-pragmatic-s(1) AND id:b k-pragmatic-s(1)",
            "id:a k-pragmatic-s(1) OR id:b k-pragmatic-s(1)",
            "id:a k-pragmatic-s(1) NOT id:b k-pragmatic-s(1)",
            "id:a k-pragmatic-s(1) AND id:b k-pragmatic-s(1) OR id:c k-pragmatic-s(1)",
            "id:a k-pragmatic-s(1) OR id:b k-pragmatic-s(1) AND id:c k-pragmatic-s(1)",
            "id:a k-pragmatic-s(1) AND (id:b k-pragmatic-s(1) OR id:c k-pragmatic-s(1))",
            "(id:a k-pragmatic-s(1) OR id:b k-pragmatic-s(1)) AND id:c k-pragmatic-s(1)",
            "(id:a k-pragmatic-s(1) AND id:b k-pragmatic-s(1)) OR (id:c k-pragmatic-s(1) NOT id:d k-pragmatic-s(1))",
            "id:a k-pragmatic-s(1) AND id:b k-pragmatic-s(1) AND id:c k-pragmatic-s(1)",
            "id:a k-pragmatic-s(1) OR id:b k-pragmatic-s(1) OR id:c k-pragmatic-s(1)",
            "NOT id:x k-pragmatic-s(1)",
        ];
        for input in filter_inputs.iter().chain(comp_inputs.iter()) {
            let spec = parse_ok(input);
            let serialized = serialize(&spec);
            let spec2 = parse(&serialized).unwrap_or_else(|e| {
                panic!("round-trip failed for '{input}' → '{serialized}': {e}")
            });
            assert_eq!(
                spec.steps.len(),
                spec2.steps.len(),
                "step count mismatch for round-trip of '{input}' → '{serialized}'"
            );
            // Verify the re-serialized form matches (serialize is idempotent)
            let serialized2 = serialize(&spec2);
            assert_eq!(
                serialized, serialized2,
                "double round-trip mismatch for '{input}': first='{serialized}', second='{serialized2}'"
            );
        }
    }

    #[test]
    fn parenthesized_group_with_anchored_pipelines() {
        // Anchored pipelines inside grouping parens
        let spec = parse_ok("(id://cat-a k-pragmatic-s(1)) AND (id://cat-b k-pragmatic-s(1))");
        assert_eq!(outer_op(&spec), CompositionOp::And);
    }

    #[test]
    fn serializer_emits_minimal_parens() {
        // A AND B OR C: AND binds tighter than OR, no parens needed.
        // Note: k-pragmatic-s(1) round-trips as uses(1) (named shorthand).
        let spec =
            parse_ok("id:a k-pragmatic-s(1) AND id:b k-pragmatic-s(1) OR id:c k-pragmatic-s(1)");
        let serialized = serialize(&spec);
        assert!(
            !serialized.contains('(') || !serialized.contains("(id:"),
            "should not wrap AND in parens: {serialized}"
        );
        assert!(
            serialized.contains(" AND ") && serialized.contains(" OR "),
            "should contain both operators: {serialized}"
        );

        // A OR B AND C: AND binds tighter, no parens needed.
        let spec2 =
            parse_ok("id:a k-pragmatic-s(1) OR id:b k-pragmatic-s(1) AND id:c k-pragmatic-s(1)");
        let serialized2 = serialize(&spec2);
        assert!(
            !serialized2.contains("(id:"),
            "should not wrap AND in parens: {serialized2}"
        );

        // A AND (B OR C): parens ARE needed because OR is lower precedence inside AND.
        let spec3 =
            parse_ok("id:a k-pragmatic-s(1) AND (id:b k-pragmatic-s(1) OR id:c k-pragmatic-s(1))");
        let serialized3 = serialize(&spec3);
        assert!(
            serialized3.contains("(id:"),
            "should wrap OR in parens: {serialized3}"
        );

        // Verify all three round-trip cleanly
        for s in [&serialized, &serialized2, &serialized3] {
            let rt = serialize(&parse(s).unwrap());
            assert_eq!(s, &rt, "round-trip mismatch: '{s}' → '{rt}'");
        }
    }
}
