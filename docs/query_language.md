---
version = "0.1"
title = "noet Query Language"
status = "Draft"
---

# noet Query Language

The textual query grammar for a compiled BeliefBase. This document is the
user-facing reference: it is what you type into the viewer `?q=` parameter, a
`{query}` directive body, or the MCP `query` tool's `query_string` field.

The grammar surfaces the `NodeFilter` and `Traversal` primitives of the formal
query algebra. For the algebra itself — the tape, scoring, composition semantics,
the video-camera model — see [`design/core/query_model.md`](design/core/query_model.md).
For the MCP tool surface see [`mcp.md`](mcp.md).

---

## 1. Where the Grammar Is Used

This document defines the textual query language that surfaces the `NodeFilter` and
`Traversal` primitives to users. The same grammar is used across three surfaces:

1. **Viewer URL** — the `?q=` GET parameter. View configuration (sort, display mode)
   travels as **sibling URL parameters** (`&view=connectivity&sort=tfidf`) and is NOT
   part of the query string.
2. **MyST directive** — the `{query}` directive body. View configuration is expressed
   as directive options (`:view:`, `:sort:`, `:max-rows:`) — not embedded in the query.
3. **MCP tool** — the `query_string` field in MCP tool arguments.

All three surfaces share the same query grammar. View configuration is always
supplied by the surface layer, never embedded in the query string.

The parser (`src/query/parser.rs`) is a recursive descent parser over a pre-tokenised
vector — not regex-based, unlimited lookahead.

**Design constraint — URL safety**: The query string must survive as a raw `?q=`
value without percent-encoding. Characters `<`, `>`, `@`, `[`, `]`, `{`, `}` are
avoided; sets use parentheses `(val1,val2)` instead of braces. Only `"quoted titles"`
require `%22` encoding, and those are deferred from the current MVP.

---

## 2. Quick Start

The three patterns that cover most use cases:

```
-- 1. Text search (bare colon = search all indexed fields)
:authentication
:"auth flow"                -- multi-word: must be quoted
title:oauth                 -- scope to a specific field

-- 2. Traversal from an anchor
id://priority-high uses(1)             -- what does priority-high depend on?
composed_of(3)                         -- 3-level section submap, root→leaves (implicit anchor)

-- 3. Multi-anchor traversal
KEYS(bref:abc,bref:def) composed_of(1) -- multiple starting nodes

-- 4. Composition: gap analysis
id://category k-pragmatic-s(1)
NOT
id://review-doc o-pragmatic-k(2)
```

In a `{query}` directive, omit the `id://` anchor to pin to the current document:

````md
```{query}
:view: depth0
:caption: What links here
k-pragmatic-s(1)
```
````

---

## 3. Cookbook

Common query patterns. All examples use the textual grammar; they work unchanged in
the viewer `?q=` parameter, `{query}` directive bodies, and MCP `query_string`.

**What links to this document?** _(implicit anchor — use in a `{query}` directive)_
```
uses(1)
```

**What depends on this document?**
```
used_by(1)
```

**Full section submap from a network root** _(root→leaves)_
```
id://my-network composed_of(*)
```

**Navigate to the root/container of a node** _(leaf→root)_
```
component_of(1)
```

**Multiple starting nodes** _(multi-anchor)_
```
KEYS(bref:abc,bref:def) composed_of(1)
```

**Corpus-wide search with explicit seed**
```
CORPUS() :authentication
```

**All documents with a specific schema**
```
schema == procedure
```

**Find documents matching a text search**
```
:authentication
title:"oauth flow"          -- multi-word, field-scoped
```

**Gap analysis: high-priority items with no review coverage**
```
id://priority-high k-pragmatic-s(1)
NOT
id://review-doc o-pragmatic-k(2)
```

**Cross-network: nodes in network B reachable from network A via pragmatic edges**
```
id://network-a ->section(*) THEN sk-pragmatic-n
AND
id://network-b ->section(*)
```

**MapsTo coverage: what does this section claim to cover?**
```
covers(1)
```

**Nodes with a specific payload field**
```
payload.status == open
payload.priority > 3
metadata.git.branch exists
```

**Nodes with no outgoing pragmatic edges** _(inverted traversal)_

The `!` prefix inverts a traversal into an existence filter: instead of
returning the output nodes, it returns input nodes that produced NO
output. This is the per-node existence gate.
```
-- Nodes in the submap that use nothing (no outgoing pragmatic edges):
id://my-network composed_of(*) !uses(1)

-- Inverse: nodes that DO use something.
-- Subtract non-consumers from the full set:
id://my-network composed_of(*)
NOT
(id://my-network composed_of(*) !uses(1))

-- Full traversal syntax also works:
id://my-network composed_of(*) !k-pragmatic-s(1)
```

---

## 4. NodeFilter Expressions

A `NodeFilter` stage down-selects the current node set based on node-intrinsic
properties. No traversal — output is a scored subset of the input set.

A filter stage produces one or more `ProjectionStep` values. Boolean composition
(`AND`/`OR`/`NOT`) is represented as `StepOperation::Compose` at the projection level,
not as a separate filter type (see `query_model.md` §5.1).

**Formal grammar** (EBNF):

```
filter_stage = filter_or
filter_or    = filter_and ('OR' filter_and)*
filter_and   = filter_not ('AND' filter_not)*
filter_not   = 'NOT' filter_atom | filter_atom
filter_atom  = '(' filter_stage ')'
             | predicate
             | text_match

-- Text search (soft-scored TF-IDF) — field prefix always required
text_match   = prop_path ':' WORD          -- single-word term, unquoted
             | prop_path ':' QUOTED         -- multi-word term, must be quoted
             | ':' WORD                    -- shorthand: expands to text:WORD
             | ':' QUOTED                  -- shorthand: expands to text:QUOTED

-- Property predicate (hard boolean)
predicate    = prop_path '==' value
             | prop_path '!=' value
             | prop_path '>'  number
             | prop_path '<'  number
             | prop_path '>=' number
             | prop_path '<=' number
             | prop_path 'in' '(' WORD (',' WORD)* ')'
             | prop_path 'matches' QUOTED
             | prop_path 'contains' value
             | prop_path 'exists'

prop_path    = WORD     -- any dotted path: title, payload.status, metadata.git.branch
value        = QUOTED | WORD
number       = WORD (parsed as f64)
```

**Disambiguation** after consuming `prop_path`:

| Next token          | Interpretation          |
|---------------------|-------------------------|
| `:`                 | text_match (field:term) |
| `==` `!=`           | predicate               |
| `>` `<` `>=` `<=`  | numeric predicate       |
| `in`                | set predicate           |
| `matches`           | regex predicate         |
| `contains`          | predicate               |
| `exists`            | predicate               |
| anything else       | **parse error**         |

**TextMatch is always explicit.** Bare words and bare quoted strings are parse
errors. Use `text:term` (multi-field, the common default), `title:term`,
`schema:term`, etc. Any property path is valid as the field — the evaluator
determines what each path resolves to.

**`text:` is the multi-field alias** — `text:auth` searches all indexed text
fields (title + content). `title:auth` and `content:auth` scope to specific
fields. No fallback: an unknown field name simply searches that property
path; if the path doesn't exist in the data, TF-IDF scores zero.

**`:term` shorthand** — a leading colon with no field name expands to `text:term`.
`:authentication` ≡ `text:authentication`. The canonical serialised form is always
`text:term`; `:term` is input sugar only (useful in the omni-bar or quick queries).

**`NOT` semantics**: within a filter stage, `NOT` is a **unary prefix** that
wraps the atom in `Compose(pass_all, Not, atom)`. Binary `NOT` between two
pipelines is a **query-level** operator (§7).

**Set syntax**: `kind in (Document,Symbol)` — parentheses, comma-separated,
URL-safe. Braces `{}` are not used (require percent-encoding).

Score algebra for `Compose`:
```
Compose(And, left, right) →  min(score(left),  score(right))
Compose(Or,  left, right) →  max(score(left),  score(right))
Compose(Not, left, right) →  score(left) if score(right) = None, else None
```

Examples:
```
text:authentication                   -- TextMatch(text, "authentication")
title:"auth flow"                     -- TextMatch(title, "auth flow")  [multi-word, quoted]
title:auth AND schema:procedure       -- Compose(And, TextMatch(title,auth),
                                      --              TextMatch(schema,procedure))
schema == procedure                   -- Predicate(schema, Eq, "procedure")
kind in (Document,Symbol)            -- Predicate(kind, In, {Document, Symbol})
payload.priority > 3                 -- Predicate(payload.priority, Gt, 3.0)
metadata.git.branch exists           -- Predicate(metadata.git.branch, Exists)
NOT schema:procedure                 -- Compose(Not, pass_all, TextMatch(schema,procedure))
foo:bar                              -- TextMatch(foo, "bar")  [custom/payload field]
authentication                       -- PARSE ERROR: use text:authentication
```

**Omni-bar note**: the `text:` field prefix and the explicit TextMatch requirement
mean query strings and plain-text search inputs are structurally distinct. Surface
layers (viewer omni-bar, MCP input) use a pre-parser heuristic to decide whether
to call `parse()` or construct a `TextMatch` spec directly from the raw input.

---

## 5. Traversal Expressions

A `Traversal` stage maps the current node set to a new node set by traversing one hop
through relations. The full form is:

```
INPUT_ROLES-KIND_SET-OUTPUT_ROLES(DEPTH)
```

**`INPUT_ROLES`** — which roles the input node must occupy on the matched relation.
One or more of `s k o n` concatenated (AND semantics: node must occupy ALL listed roles
simultaneously). `n` is shorthand for all three. See §8 for the role model.

**`KIND_SET`** — OR filter on the relation's WeightSet. The relation must carry at
least one of the listed kinds. `pragmatic,epistemic` matches any relation that has a
Pragmatic or Epistemic weight (or both). `*` matches any kind.

> **AND on kinds**: To require a relation that carries *both* Pragmatic and Epistemic
> weights simultaneously, use explicit composition: `s-pragmatic-k AND s-epistemic-k`.
> This is intentionally verbose — the common case is OR ("follow any of these edge
> types"), not AND ("find edges typed in multiple dimensions at once").

**`OUTPUT_ROLES`** — which nodes to resolve to from each matched relation. One or more
of `s k o n` concatenated (OR semantics: the output is the union of nodes occupying any
listed role). `n` is shorthand for all three roles.

**Terminal identification** (roots, leaves) is a `TapeFn` variant
(`Terminal`), not a traversal declaration.  The `TERMINAL` keyword
in the surface grammar (§7) maps to `TapeFn::Terminal`.  See `query_model.md` §5
for the `TapeFn` enum and §6.2 for the tape API.

**Validity**: the expression must be capable of producing nodes distinct from the input.
`s-...-s` (source in, source out) is a degenerate self-loop and is rejected by the
parser. Any expression where at least one input role differs from at least one output role
is valid.

**`(DEPTH)`** — lens parameter, default `(1)`. Comma-separated arguments
inside parentheses. Two argument types:

- **Count** (bare number or `*`): `(N)` iterates up to N times, following
  any matching edge. `(*)` = unbounded. A node visited at depth `d` is
  not revisited at `d+1`.
- **Edge filter** (`property:pattern`): at each hop, follow only edges
  whose weight property matches the pattern. If both count and filter
  are present, the filter applies at every hop for that many iterations.

The two arguments compose: `(3, idx:0)` = three hops, always following the
first child. `(*, idx:0)` = chase first children until exhaustion. Count
alone `(3)` follows any edge. Filter alone `(idx:0)` implies count `(1)`.

Parentheses are used instead of curly braces for URL safety.

> ⚠ **`(*)` requires explicit opt-in.** Unbounded depth composed with `NOT`/`Difference`
> enters undecidable territory (Trakhtenbrot 1950). The parser warns when `(*)` appears
> in a `Difference` or `NOT` context. See `query_model.md` §4 and §13.4.

**Edge filter syntax** — `property:pattern` where:

- **`path:`** — section-edge traversal guided by `WEIGHT_DOC_PATHS`. The
  pattern is `/`-delimited (documents) and `#`-delimited (sections within
  a document). Supports `*` (single segment wildcard) and `**` (recursive
  — zero or more segments). The parser splits on `/` and `#` to produce
  a sequence of per-hop edge predicates. `path:doc/**#sec-a` = "navigate
  to `doc`, then descend any number of levels, then match `sec-a`."
- **`idx:`** — select edge(s) by sort-key position (`WEIGHT_SORT_KEY`).
  `idx:0` = first child, `idx:1..3` = edges at positions 1 and 2.
- **Any other name** — literal match against that edge `Weight.payload`
  key. `owned_by:abc12` matches `WEIGHT_OWNED_BY == "abc12"`.

**`path:` multi-hop expansion**: `path:a/b#c` desugars to three sequential
single-hop traversals, each with the appropriate segment as edge filter.
`**` at any position desugars to a `(*)` count step (unbounded) between
the adjacent guided steps. The evaluator chains these internally.

Examples:
```
s-pragmatic-k                    -- depth=1, any edge
k-pragmatic-s                    -- I am sink, give me sources
sk-pragmatic-o                   -- I am source or sink, give me owners
o-pragmatic-k                    -- I am owner, give me sinks (MapsToTraversal)
n-pragmatic-n                    -- full three-party neighborhood
n-section,pragmatic-k            -- Section OR Pragmatic, resolve to sink
s-*-k(3)                         -- 3 hops, any kind, any edge
s-section-k(*)                   -- unbounded Section traversal ⚠
s-section-k(path:doc.md)         -- one hop, doc_paths == "doc.md"
s-section-k(path:doc.md#sec-a)   -- two hops (doc then section)
s-section-k(path:doc/**)         -- doc + all descendants
s-section-k(path:**/sec-a)       -- sec-a at any depth
s-section-k(*, idx:0)            -- unbounded, first child each hop
s-section-k(3, idx:0)            -- 3 hops, first child each hop
s-epistemic-k(idx:0)             -- first epistemic edge (depth=1)
s-epistemic-k(idx:1..3)          -- edges at positions 1 and 2
s-pragmatic-k(owned_by:abc12)    -- edges owned by bref abc12
```

Named shorthands (canonical — derived from DIRECTIVES verb names and TraversalSpec):
```
-- Section traversals (S content — structural containment)
composed_of(N)     ≡  k-section-s(N)           -- root→leaf: what does this consist of?
consists_of(N)     ≡  k-section-s(N)           -- (backward-compatible alias for composed_of)
component_of(N)    ≡  s-section-k(N)           -- leaf→root: what is this a component of?
roots()            ≡  s-section-k(*) TERMINAL  -- all root nodes
leaves()           ≡  k-section-s(*) TERMINAL  -- all leaf nodes

-- Epistemic traversals (N content — normative coupling; EMO §7.2)
constrained_by(N)  ≡  k-epistemic-s(N)         -- what normatively constrains this?
constrains(N)      ≡  s-epistemic-k(N)         -- what does this normatively constrain?
draws_from(N)      ≡  k-epistemic-s(N)         -- (alias for constrained_by)
underlies(N)       ≡  s-epistemic-k(N)         -- (alias for constrains)
covers(N)          ≡  o-epistemic-sk(N)        -- owner→edge endpoints (MapsTo/traceability)

-- Pragmatic traversals (P content — procedural/operational)
uses(N)            ≡  k-pragmatic-s(N)         -- what does this operationally use?
implements(N)      ≡  k-pragmatic-s(N)         -- (alias for uses)
used_by(N)         ≡  s-pragmatic-k(N)         -- what operationally uses this?

-- Structural
halo()             ≡  n-*-n(1)                 -- immediate full neighborhood

-- Inverted traversals ("!" prefix): existence filter.
-- Returns input nodes that produce NO output (per-node check).
!constrained_by(1) ≡  !k-epistemic-s(1)        -- nodes with no normative constraints
!constrains(1)     ≡  !s-epistemic-k(1)        -- nodes that constrain nothing
!uses(1)           ≡  !k-pragmatic-s(1)        -- nodes that use nothing
!used_by(1)        ≡  !s-pragmatic-k(1)        -- nodes nothing depends on
!composed_of(1)    ≡  !k-section-s(1)          -- leaf nodes (no children)
```

For edge-filter depth specs (`path:`, `idx:`) use the full traversal form directly:
```
s-section-k(path:doc.md)    -- one hop, matching edge path
s-section-k(*, idx:0)       -- unbounded, first edge at each hop
```

---

## 6. Anchored Queries and Seed Syntax

An anchor sets the seed `TapeFn` on a step. There are two forms:

**Bare anchor** (sugar for single-key seed) — a NodeKey string at the start
of a pipeline. Produces `TapeFn::Keys(vec![key])` on the first step.
NodeKey strings follow a URL-based format with four schemes: `id:`, `bref:`,
`bid:`, and `path:` (see "Node Identity: Multi-ID Triangulation" in
[`architecture.md`](design/core/architecture.md) for details).

```
id://priority-high k-pragmatic-s(1)    -- hierarchical id (network-scoped)
bref:abc123def456 ->section(3)         -- non-hierarchical bref
id:my-node uses(1)                     -- non-hierarchical id (implicit network)
```

**Explicit seed functions** — `KEYS(...)`, `CORPUS()`, `BIDS(...)` as callable
syntax. Arguments are comma-separated NodeKey strings. Can appear at any
position in the pipeline (mid-pipeline re-seeding):

```
KEYS(bref:abc,bref:def) composed_of(1)   -- multi-anchor
CORPUS() :authentication                  -- explicit corpus seed
KEYS(id://doc-a) composed_of(1) AND KEYS(id://doc-b) uses(1)
```

A bare single NodeKey is sugar for `KEYS(key)` — backward compatible
with existing single-anchor queries.

When no anchor or seed function is given, the first step has
`TapeFn::Then(None)` (the default). The query is **context-dependent** —
the caller must inject a concrete seed `TapeFn` before evaluation:
- **Directive**: injects `TapeFn::Bids([doc_bid])` (current document)
- **Viewer**: injects `TapeFn::Bids([route_bid])` or `TapeFn::Corpus`
- **MCP**: may reject as an error or default to `TapeFn::Corpus`

**Quoted title anchors** (`"Design Review" o-pragmatic-k(2)`) are **deferred** from
the current implementation. Quoted strings in filter position are treated as content
text-match terms.

**Multi-anchor compositions**: each branch of `AND`/`OR`/`NOT` can have its own
seed `TapeFn`. The parser emits the seed as the `TapeFn` on the branch's first
step — no special `Subject` handling needed.

---

## 7. Full Query Expression

The query expression has two distinct combining operations:

- **Pipeline** (sequential) — the output of one stage feeds the next. Written as
  juxtaposition (adjacent stages) or with the optional `THEN` keyword for clarity.
  Maps to `Vec<ProjectionStep>` in the QuerySpec.
- **Composition** (parallel) — two independently-evaluated pipelines whose results are
  combined via set algebra. Written with `AND`, `OR`, or `NOT`. Maps to `And`/`Or`/
  `Difference` in the QuerySpec.

```
SEED_FN      = 'KEYS' '(' KEY (',' KEY)* ')'
             | 'CORPUS' '(' ')'
             | 'BIDS' '(' BID (',' BID)* ')'
ANCHOR       = ('id://' | 'id:' | 'bref:' | 'bid:') WORD   -- sugar for KEYS(key)

-- Continuation: shared by PIPELINE, COMP_ATOM groups, and QUERY.
-- A TAPE_FN may appear with or without a following STAGE.
-- Without a STAGE, it produces an Identity step (pass-through).
CONTINUATION = (TAPE_FN STAGE | TAPE_FN | STAGE)*

PIPELINE     = [ANCHOR | SEED_FN] STAGE CONTINUATION

-- Composition: precedence-based recursive descent.
-- OR (lowest) < AND/NOT (same level) < unary NOT (highest).
COMP_OR      = COMP_AND ('OR' COMP_AND)*
COMP_AND     = COMP_NOT (('AND' | 'NOT') COMP_NOT)*
COMP_NOT     = 'NOT' COMP_NOT | COMP_ATOM
COMP_ATOM    = '(' COMP_OR CONTINUATION ')' | PIPELINE

QUERY        = COMP_OR CONTINUATION

TAPE_FN      = "THEN" ["(" STEP_REF ")"]               -- TapeFn::Then
             | "FOLD" "(" SET_OP ["," RANGE] ")"        -- TapeFn::Fold
             | "TERMINAL" ["(" RANGE ")"]               -- TapeFn::Terminal
             | "ORPHAN" ["(" RANGE ")"]                 -- TapeFn::Orphan
SET_OP       = "UNION" | "INTERSECT" | "LDIFF" | "RDIFF" | "SYMDIFF"
RANGE        = STEP_REF "," STEP_REF
STEP_REF     = LABEL | INDEX
```

**Composition precedence.** Composition operators follow standard boolean
precedence: OR binds loosest, AND and binary NOT share the same level, unary
NOT binds tightest. Parenthesized groups override precedence. The `(` token is
unambiguous at the composition level: argument parens are consumed inside
stages and seeds, so `(` at a `COMP_ATOM` position is always a grouping paren.

| Precedence | Operators | Associativity |
|-----------|-----------|---------------|
| Lowest    | `OR`      | Left          |
| Medium    | `AND`, binary `NOT` | Left  |
| Highest   | Unary `NOT` | Prefix      |

Examples of precedence:
```
-- AND binds tighter than OR:
id://a trav OR id://b trav AND id://c trav
= id://a trav OR (id://b trav AND id://c trav)

-- Parens override:
id://a trav AND (id://b trav OR id://c trav)

-- Unary NOT:
NOT id://review-doc ->section(3)
= pass_all NOT id://review-doc ->section(3)
```

**Filter-level vs composition-level operators.** Within a single filter
stage, AND/OR/NOT are consumed by the filter parser (§4) with their
own precedence rules. Composition-level operators apply between
independently-anchored or multi-stage pipelines. The filter parser is
greedy: `title:a OR title:b` is a single filter-OR stage, not two
pipelines composed at the query level.

**Continuation.** The `CONTINUATION` production is shared by `PIPELINE`,
`COMP_ATOM` (inside grouping parens), and `QUERY` (top level). It is
implemented by a single method (`parse_continuation_stages`) that parses
a sequence of tape functions and/or stages.

Bare juxtaposition (no explicit operator between stages) defaults to
`THEN` — the previous entry's output feeds the next stage.  `THEN` with
an explicit `STEP_REF` references a specific labeled entry:
`THEN("balance")` feeds from the last entry labeled `"balance"`.

**Terminal tape functions.** A `TAPE_FN` without a following `STAGE`
produces an Identity step (pass-through). This allows terminal folds
like `composed_of(*) FOLD(UNION)` and chained tape functions like
`FOLD(UNION) THEN used_by(1)`. The Identity step applies the tape
function's input selection (fold, terminal filter, etc.) and passes
the result forward.

Examples:
```
-- Sequential (THEN is implicit):
s-section-k(3) s-pragmatic-k(1)

-- Explicit THEN referencing a labeled step:
s-section-k(3) THEN("balance") s-pragmatic-k(1)

-- FOLD with UNION across prior entries:
s-section-k(*) FOLD(UNION) s-pragmatic-k(1)

-- Terminal FOLD — collapse multi-key traversal into a single set:
KEYS(id:a,id:b) composed_of(*) FOLD(UNION)

-- Chained tape functions — fold then continue:
KEYS(id:a,id:b) composed_of(*) FOLD(UNION) THEN used_by(1)

-- Terminal FOLD inside a composition arm:
id:a uses(1) AND (KEYS(id:b,id:c) composed_of(*) FOLD(UNION))

-- TERMINAL — roots of a section traversal feed next stage:
s-section-k(*) TERMINAL s-pragmatic-k(1)

-- ORPHAN — nodes with no section edges:
s-section-k(1) ORPHAN kind == "Document"

-- FOLD with branch labels — extract intersection after composition:
id://cat-a k-pragmatic-s(1) AND id://cat-b k-pragmatic-s(1)
FOLD(INTERSECT, "0.L", "0.R") s-section-k(1)

-- Post-composition continuation — left-unique items:
id://cat-a k-pragmatic-s(1) NOT id://cat-b k-pragmatic-s(1)
FOLD(LDIFF, "0.L", "0.R") k-pragmatic-s(1)

-- Continuation inside grouping parens:
((id:a uses(1) NOT id:c uses(1)) THEN used_by(1)) AND id:d uses(1)
```

After a composition, the query may continue with additional stages via
`CONTINUATION`. `FOLD(op, left_label, right_label)` selects which subset
of the composition to feed forward (see `query_model.md` §5.3.1). Without an
explicit Fold, `THEN` passes the operator's `result` (intersection for AND, union
for OR, left difference for NOT).

A `STAGE` is either a `NodeFilter` expression (§4) or a `Traversal` expression
(§5). Stage boundary is implicit: a role sigil followed by `-` opens a Traversal;
`->` or `<-` opens a named shorthand; all other tokens enter NodeFilter parsing.
Single-token lookahead, no backtracking.

Composition operators combine pipelines with the `Score` algebra (`min`/`max`/mask).
Each side of `AND`/`OR`/`NOT` is an independently anchored pipeline — the two cameras
in the stereoscopic model (`query_model.md` §5.3).

Examples:
```
// NodeFilter only:
title:authentication AND schema:procedure NOT basic

// SectionSubmap from anchor:
id://review-doc ->section(3)

// Pipeline — section submap, then traverse pragmatic edges from result:
id://network_a ->section(*) THEN sk-pragmatic-n

// Same pipeline, implicit THEN (juxtaposition):
id://network_a ->section(*) sk-pragmatic-n

// Pragmatic in-neighbors of a category node:
id://priority-high k-pragmatic-s(1)

// MapsToTraversal — owner resolves to sinks:
id://review-doc o-pragmatic-k(2)

// Category join — stereoscopic composite of two cameras:
id://priority-high k-pragmatic-s(1)
AND
id://review-doc o-pragmatic-k(2)

// Complement — categorized items with no review coverage:
id://priority-high k-pragmatic-s(1)
NOT
id://review-doc o-pragmatic-k(2)

// Cross-network edges: nodes in network_b connected to network_a
// via pragmatic or epistemic edges (B-side endpoints, A-side path in tape):
id://network_a ->section(*) THEN sk-pragmatic,epistemic-n
AND
id://network_b ->section(*)

// Same query, A-side endpoints (swap the intersection):
id://network_b ->section(*) THEN sk-pragmatic,epistemic-n
AND
id://network_a ->section(*)

// Owner or sink of a Pragmatic relation, resolve to source:
ko-pragmatic-s

// Source or sink, resolve to all neighbors, any kind:
sk-*-n
```

> **Stage references (future reserve)**: A `$name = PIPELINE` binding syntax would
> allow naming a pipeline result and reusing it in multiple composition arms without
> repeating the subexpression. This is deferred — no current use case justifies the
> complexity (symbol table, scoping, evaluation order, multi-statement URL encoding).
> The `$` sigil is reserved for this purpose and must not appear in `WORD` tokens. A
> query optimizer can detect and cache repeated subexpressions internally without
> exposing binding in the grammar.

---

## 8. Role Occupancy Model

A relation has three named participant roles: **source** (`s`), **sink** (`k`), and
**owner** (`o`). A node may occupy multiple roles on the same relation instance
simultaneously:

```
Self-owned relation (source is owner):
    source node  — occupies s AND o
    sink node    — occupies k

Self-owned relation (sink is owner):
    source node  — occupies s
    sink node    — occupies k AND o

{maps_to} third-party relation:
    source node  — occupies s
    sink node    — occupies k
    owner node   — occupies o exclusively (not s or k)
```

**Role sigils** — single URL-safe letters:

| Sigil | Role   | Mnemonic |
|-------|--------|----------|
| `s`   | source | **s**ource — the more-discrete end |
| `k`   | sink   | sin**k** — the more-interconnected end |
| `o`   | owner  | **o**wner — the node that declared the edge |
| `n`   | neighbors | **n**eighbors — wildcard, matches all three roles |

`WEIGHT_OWNED_BY` is always set — it names the node that declared the relation. In the
common case it equals the source or sink bref (self-ownership). In the `{maps_to}` case
it names a third-party section node that is neither source nor sink.

Multi-role input sets are letter sequences: `sk` means "node must occupy source AND
sink simultaneously" (rare but valid). The wildcard `n` means "any combination of
`s`, `k`, `o`" — all nodes that participate in the relation in any role.

---

## 9. Tokens

```
WORD         [^\s:(),-|$?*"!=<>-]+        -- identifiers; dots OK (payload.status)
QUOTED       "..."                         -- quoted string (value literals, path args)
IDaNCHOR     id://WORD                     -- WORD may include hyphens (priority-high)
KIND         section | epistemic | pragmatic
KIND_SET     KIND(,KIND)*  |  *            -- OR-filter: any listed kind matches
ROLE_SET     one or more of {s k o n}      -- AND for input, OR-union for output
DEPTH        (N)  |  (*)                   -- N: 0-255; (*): unbounded
EDGE_FILTER  WORD:WORD                     -- property:value, e.g. idx:0 path:doc.md
KNOWN_FIELD  title | schema | kind | id | content   -- for TextMatch
```

Special multi-character tokens (detected before WORD): `->`, `<-`, `==`, `!=`, `>=`,
`<=`. The colon `:` is emitted as a distinguishable pseudo-token within the word stream
(field:term and edge-filter syntax). `AND`, `OR`, `NOT`, `THEN`, `FOLD`, `TERMINAL`,
`ORPHAN` are case-sensitive uppercase-only keywords. Word operators `in`, `matches`,
`contains`, `exists` are lowercase-only and only interpreted as operators by
contextual lookahead.

---

## 10. View Configuration

View configuration (sort order, display mode, column selection) is **not embedded in
the query string**. It is supplied by each surface through its own idiomatic mechanism:

| Surface | View config mechanism |
|---------|----------------------|
| Viewer URL | Sibling query params: `?q=QUERY&view=connectivity&sort=tfidf&max_rows=50` |
| MyST directive | Directive options: `:view:`, `:sort:`, `:max-rows:`, `:caption:`, `:columns:` |
| MCP tool | Separate JSON fields alongside `query_string` |

The `view` key selects a renderer from the `ViewRegistry` (`query_model.md` §7.1).
Built-in keys:

| Key | Description |
|-----|-------------|
| `depth0` | Node intrinsics: title, schema, kind (default) |
| `connectivity` | Connectivity matrix: In/Out per WeightKind |
| `maps_to` / `o-k` | Owner→sink traceability |
| `columns` | Explicit column list from `columns` param |

All other params are passed as an opaque `toml::Table` to the renderer via
`ViewRenderer::spec()`. The `sort` param uses `SortSpec` string form
(`section_order`, `tfidf`, `path_length`, `intersection_cardinality`, or composite
`tfidf:0.7,path_length:0.3`).

---

## 11. Canonical Form

The query string is the **canonical serialization** of a `QuerySpec` step pipeline
(`src/query/parser.rs`). The same grammar is used across all three surfaces. The
serializer produces canonical form: `parse(serialize(spec)) == spec`. Parentheses
are emitted only when needed to preserve non-default precedence (§13); `:term`
shorthand is always serialised as `text:term` (§4).

The canonical query string is also what annotation records anchor to: a record
points at a query that selects what the record is about, and the record's liveness
is checked by re-running that query (see
[`design/identity/content_versioning.md`](design/identity/content_versioning.md) §4
for the anchor model). Write specs you would want to re-run: prefer `id://` seeds
over `bref:` seeds where the document has an `id`, and prefer the narrowest
traversal that still selects the target.

---

## 12. Surface Bindings

**Viewer URL (GET parameter)**

Query string and view config are separate URL parameters:
```
https://example.com/site/#/doc
  ?q=id://priority-high+k-pragmatic-s(1)+AND+id://review-doc+o-pragmatic-k(2)
  &view=connectivity
  &sort=intersection_cardinality
```

The `?q=` value is the raw query string (juxtaposition uses `+` for space). View
config travels as sibling `&key=value` params. The viewer:
- Reconstructs full control state from `?q=` on load
- Serializes control state back to `?q=` on every change
- Stores the query string in `localStorage` as fallback
- `?q=` takes precedence on reload

**MyST directive (static embedding)**

A `{query}` directive embeds a live-rendered query result in a source document.
The directive body is the raw query string; view configuration is directive options:

````
```{query}
:view: connectivity
:sort: section_order
:max-rows: 50
:caption: Applicable Requirements
id://review-doc k-pragmatic-s(1)
```
````

At HTML generation time the compiler evaluates the query against the compiled
BeliefBase and renders the result as an HTML table (or other render mode). The
rendered output is a static snapshot — it updates on recompilation, not at view time.

**MCP tool (programmatic)**

The MCP `query` tool accepts either a textual query string or a structured JSON
`QuerySpec`:

```json
{ "query_string": "id://priority-high k-pragmatic-s(1)" }
```

```json
{
  "expression": {
    "steps": [
      { "input": { "Bids": ["550e8400-e29b-41d4-a716-446655440000"] },
        "operation": "Identity" }
    ]
  }
}
```

When `query_string` is present it is parsed via `parse()` and takes precedence over
`expression`. `QuerySpec` has a single field `steps`: an array of
`{ label, input, operation }` objects. The `input` field is a `TapeFn` (one of
`Then`, `Fold`, `Terminal`, `Orphan`, or a seed variant: `Bids`, `Keys`, `Corpus`,
`DocumentNodes`). The `operation` field is a `StepOperation` (`Filter`, `Traverse`,
`Compose`, or `Identity`). See `query_model.md` §3 for the full schema.

The named MCP tools (`get_submap`, `get_maps_to_traceability`, etc.) are shorthand
compositions — each constructs a `QuerySpec` internally and delegates to the unified
evaluator. The raw `query` tool exposes the full surface for arbitrary queries.

---

## 13. Parser Rules

See `query_model.md` §10.3 for full parser implementation rules (disambiguation,
token set, keyword normalisation). The key points for authors:

- Operator keywords are **case-insensitive**: `and`/`AND`/`And` all work.
- TextMatch always requires an explicit field prefix: `title:auth`, `text:auth`, `:auth`.
  Bare words without a field prefix are a parse error.
- Single-role self-loops (`s-...-s`) are rejected. `n-pragmatic-n` is valid.
- **Composition precedence**: OR < AND/NOT < unary NOT. Parenthesized groups
  override. See §7 grammar and precedence table.
- The serializer produces canonical form: `parse(serialize(spec)) == spec`.
  Parentheses are emitted only when needed to preserve non-default precedence.
