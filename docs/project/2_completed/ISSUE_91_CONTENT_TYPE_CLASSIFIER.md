# Issue 91: N/S/P/R Content-Type Classifier (Proof of Concept)

**Priority**: LOW
**Estimated Effort**: 1.5 days
**Dependencies**: None (uses existing tokenizer and index-building infrastructure)
**Blocks**: Issue 85 (3D Credibility Map — uses `content_profile` as seed
positions for force-directed layout)

## Summary

Engineering text carries mixed content types — normative (N), structural (S),
procedural (P), and as-run record (R) — in varying proportions. The noet
parser already atomizes content into nodes that approximate "thoughts": a
requirement paragraph, a design description, a procedure step, a test result.
A classifier that scores each node's N/S/P/R proportions using grammatical
voice/tense detection and token-shape heuristics would enable automated mesh
quality assessment, process compliance checking, content coverage
visualization, and seed positions for the 3D credibility map (Issue 85).

The key insight (documented in [wp-model-ontology] §3.5) is that the
content-type signal is in the **voice and tense, not in the nouns**. Passive
voice + modals → N, imperative + causal + future → P, past tense → R. This
makes the classifier domain-independent without recalibration.

## Goals

- Classify content at **node granularity** — the parser's existing
  atomization is the right segmentation level
- Score four **independent axes** (N, S, P, R) — each on [0, 1], NOT a
  normalized simplex. A node can score high on all three spatial axes
  simultaneously. R detects temporal anchoring — content bound to a
  specific observation event. High R flags content susceptible to
  expiration; mixed N+R or S+R flags content that should be decomposed
  to separate model from observation.
- Score nodes using **grammatical signal words and token-shape heuristics**
  — domain-independent, no corpus-specific training needed
- Expose through MCP tools and search results

## Architecture

### Classification unit: the BeliefNode

The noet codec already segments content into nodes at approximately the right
granularity for content-type classification. A section heading with its body
paragraph, a numbered requirement, a list item in a procedure, a test result
entry — each is a `BeliefNode` with `payload["text"]`. The section-edge
hierarchy provides nesting context.

No additional segmentation is needed. The nodes are the blocks.

### Scoring: grammatical voice/tense + token-shape heuristics

The classifier uses two complementary signal channels on raw text and
stemmed tokens, with no domain-specific vocabulary:

**N (Normative)** — signal-word density + passive voice patterns:
- Signal words (stemmed): modal verbs (`shall`, `must`, `should`, `may`,
  `might`, `can`, `could`), constraint language (`within`, `exceed`,
  `limit`, `toler`, `requir`, `specifi`, `mandat`, `ensur`), negation
  (`not`, `no`, `never`, `without`)
- Passive voice bigrams on raw text: be-form + past participle
  ("is maintained", "are specified", "shall be verified")
- Descriptive text is normative — "is configured with" specifies the
  boundary of the referent as an entity of attention

**P (Procedural)** — signal-word density for dynamic language:
- Imperative verbs: `begin`, `execut`, `verifi`, `perform`, `connect`, etc.
- Causal verbs: `caus`, `trigger`, `produc`, `yield`, `propagat`
- State transitions: `transit`, `switch`, `enter`, `exit`, `chang`, `becom`
- Future tense: `will`
- Sequential markers: `then`, `next`, `step`, `first`
- Logic conjunctions: `if`, `else`, `unless`, `while`, `until`

**S (Structural)** — score is 0.0 lexically. Structural content is
identified by graph topology (what refers to what), not by distinctive
vocabulary. Issue 85's force-directed layout handles S positioning via
typed gravity on epistemic/pragmatic edge counts.

**R (Record)** — token-shape heuristics on raw text:
- Past-tense density: fraction of words ending in "-ed" (strongest
  signal — observations are reported in past tense)
- Timestamp patterns: `HH:MM:SS`, `YYYY-MM-DD`, ISO 8601 variants
- Commit hashes: 7-40 char lowercase hex with at least one digit
- Numeric density: fraction of characters that are digits
- Unit patterns: word-boundary matches for measurement units
- Colon density: key-value structure typical of data records

Scoring formula:
- `N = 0.6 × signal_word_fraction + 0.4 × passive_voice_density`
- `P = signal_word_fraction`
- `S = 0.0`
- `R = 0.30 × past_tense + 0.25 × timestamps + 0.15 × hashes + 0.15 × numeric + 0.05 × colons + 0.10 × units`

### Relationship to graph-aware positioning (Issue 85)

The graph structure carries content-type signal that pure text misses:
edge topology encodes how a node participates in the model network.
However, graph-aware positioning is inherently a global computation —
a node's placement depends on its neighbors' placements, which depend
on *their* neighbors, etc. This is exactly what a force-directed layout
solves.

Issue 85's typed gravity (section→S, epistemic→N, pragmatic→P) subsumes
what a local edge-count structural score would compute on its own: force
simulation considers transitive neighborhood influence, not just immediate
edge counts. However, the local structural score is still a useful
primitive — it provides the per-node gravity bias vector that the force
simulation consumes, and it serves as a fallback when the full force
simulation hasn't run.

This issue provides three library functions in `content_type.rs`:

| Function | Input | Output | Integrated | Consumer |
|---|---|---|---|---|
| `score_lexical` | tokens + raw text | `ContentProfile` | Yes (`terminate_stack`) | This issue |
| `score_structural` | edge counts | `ContentProfile` (R=0) | No | Issue 85 layout |
| `score_merge` | lexical + structural | `MergeResult` | No | Issue 85 layout |

`score_structural` maps a node's edge counts by `WeightKind` and
direction to an N/S/P bias vector. R is always 0 — edges encode
spatial relationships, not temporal anchoring. The mapping:

| Structural signal | Content-type implication |
|---|---|
| High outgoing epistemic edges (depends on) | N-like: this node constrains downstream designs |
| High incoming epistemic edges (depended upon) | S-like: other models reference this node as design input |
| High outgoing pragmatic edges (consumes) | P-like: this node exercises or traces to other models |
| High incoming pragmatic edges (consumed by) | S-like: procedures exercise this node |
| Owned edges (`maps_to` directives) | P-like: traceability action |

`score_merge` blends lexical and structural profiles: `merged = α ×
lexical + (1 - α) × structural`. Default α = 0.7 (text-dominant,
structure as correction). Nodes with no text get α = 0 (structure
only). Nodes with no edges get α = 1 (lexical only). The blend weight
and fallback policy are testable independently of both the classifier
and the layout engine.

`score_merge` also detects **channel divergence**: when lexical and
structural profiles disagree significantly on the same axis (e.g. high
structural N but low lexical N), it emits warning diagnostics. This
flags nodes where text content doesn't match graph role — either the
text needs revision or the structural relationships are wrong. The
per-axis divergence threshold defaults to 0.4 and is tunable via the
`NOET_CONTENT_DIVERGENCE_THRESHOLD` environment variable.

The separation:
- **Issue 91** (this issue): `score_lexical` integrated into
  `terminate_stack` → `metadata.content_profile`. `score_structural`
  and `score_merge` provided as library functions.
- **Issue 85**: calls `score_structural` + `score_merge` per-node
  during post-compile layout, uses merged profile as seed position,
  then runs force simulation → `metadata.render_position`.

### Relationship to search

Complementary, not competing. Search answers "which nodes contain this term?"
(retrieval). The classifier answers "what kind of content is this node?"
(analysis). They share the tokenizer and stemmer. The stop word list has
been refined to preserve engineering-signal words (modals, negation,
quantifiers, conditionals) that carry both search and classification value.

### Integration: `terminate_stack` in the builder pipeline

Profiles are stored in each `BeliefNode`'s `metadata` map — a TOML table
that's already serialized in the BB export. This makes profiles available
everywhere the node is available: HTML rendering, MCP tools, search results,
BB export, WASM viewer. No separate index file needed.

Classification runs inside `GraphBuilder::terminate_stack`, triggered by
`NodeUpdate` and `NodeUpsert` events in the diff stream. After
`compute_diff` produces diff events and `session_bb` absorbs them (but
before `tx.send()`), the classifier intercepts node events, scores the
`BeliefNode`'s `payload["text"]`, and writes the profile into
`node.metadata["content_profile"]` on the event's `BeliefNode` before
it is emitted downstream.

This is purely lexical — no edge context is needed, so no
relation-triggered reclassification is required. Only nodes whose text
changed (i.e. those appearing in `NodeUpdate`/`NodeUpsert` events) are
(re)classified.

```
parse_content → initialize_stack → codec.parse → push/push_relation
  → terminate_stack:
      compute_diff → session_bb absorbs events
      for each NodeUpdate/NodeUpsert in tx_events:
        1. tokenize payload["text"], extract raw text
        2. signal-word density + passive voice → N score
        3. signal-word density (imperatives/causal/transitions) → P score
        4. past tense + timestamps + hashes + numerics → R score
        5. write to node.metadata["content_profile"]
      emit events via tx (profiles flow through normal pipeline)
```

The `metadata` entry per node:
```toml
[metadata.content_profile]
n = 0.82
s = 0.00
p = 0.15
r = 0.05
```

Four independent scores, each on [0, 1]. No normalization constraint — a
node can score high on multiple axes simultaneously. S is always 0 from
the lexical channel (graph-level concern for Issue 85). R detects
temporal anchoring: high R flags content bound to a specific observation
event (susceptible to expiration); mixed spatial+R flags content that
should be decomposed to separate model from observation.

## Implementation Steps

1. Core types and scoring functions (0.5 day)
   - [x] `ContentProfile { n: f32, s: f32, p: f32, r: f32 }` struct in
         `src/shard/content_type.rs`
   - [x] Signal word sets for N and P (stemmed forms, `HashSet<&str>`)
   - [x] `fn score_lexical(tokens: &[String], raw_text: &str) -> ContentProfile`
         using signal-word density + passive voice + R heuristics
   - [x] `fn score_structural(edges: &EdgeCounts) -> ContentProfile`
         maps edge counts by WeightKind/direction to N/S/P bias (R=0)
   - [x] `fn score_merge(lexical: &ContentProfile, structural: &ContentProfile) -> MergeResult`
         α-blend with signal-availability fallback + channel divergence
         detection
   - [x] Unit tests: signal-word scoring, passive voice, past tense,
         timestamps, commit hashes, structural edge-count mapping,
         merge blend + fallback, divergence warning emission

2. Integration into `terminate_stack` (0.5 day)
   - [x] In `GraphBuilder::terminate_stack`, after `session_bb` absorbs
         diff events but before `tx.send()`: iterate `tx_events`, and
         for each `NodeUpdate`/`NodeUpsert` event, score the node's
         `payload["text"]` and write `metadata["content_profile"]`
   - [x] Stemmer stored on `GraphBuilder`, constructed once in `new()`
         and reused across all `terminate_stack` calls
   - [x] Profiles flow through the normal `tx` event pipeline — no
         additional serialization path needed
   - [x] Verify parse pipeline is unchanged (no regression in existing
         tests, no performance regression) — 30 unit tests pass, a
         production corpus build completes successfully with classify
         enabled

3. Validation (0.5 day)
   - [x] MCP `get_context` already returns `node.metadata` — profiles
         appear automatically, no MCP code changes needed
   - [x] Validate against a production corpus — spot-checked across
         content types, all scoring correctly:
         - Requirements text ("should include", "should be visible"): N=0.10 highest ✓
         - Design constraints ("will have... in order to"): N=0.11 highest ✓
         - Build process ("when enabled... sets... runs"): P=0.07 highest ✓
         - VV test record (timestamps, commit hashes, status): R=0.48 dominant ✓
         - S=0.0 on all lexical scoring (graph-level, by design) ✓
   - [x] Document observed accuracy and failure modes:
         - Polysemous stems ("first", "gener") caused P dual-firing on
           normative text — resolved by curating signal word lists
         - Code blocks produce noise — recommend skip scoring (open question)
         - Passive voice detection correctly boosts N on descriptive text

## Testing Requirements

- Unit tests for signal-word density scoring (N and P)
- Unit tests for passive voice detection and past-tense density
- Unit tests for R heuristics (timestamps, commit hashes, numeric
  density, unit patterns, prose rejection)
- Unit tests for structural scoring from known edge topologies
- Unit tests for merge blend weights, signal-availability fallback,
  and channel divergence warning emission
- Integration test: small test corpus compiled through `GraphBuilder` →
  verify `metadata.content_profile` appears on nodes with plausible values
- Regression test: existing compiler and search tests pass unchanged
- No golden-file tests on exact proportions (tuning will shift values)

## Success Criteria

- [x] Per-node lexical `ContentProfile` computed during parse for every
      content node (`metadata.content_profile`)
- [x] `score_structural` and `score_merge` available as library functions
      for Issue 85 consumption
- [x] Scoring uses grammatical voice/tense and signal words, not
      domain-specific vocabulary or corpus-relative statistics
- [x] Profiles accessible through MCP `get_context` (via `node.metadata`)
- [x] Validation on a production corpus shows content-type-dominated
      nodes score appropriately on their expected dominant type

## Risks

- **Signal-word coverage**: The curated signal word sets may not cover
  all relevant vocabulary across engineering domains.
  **Mitigation**: Signal words are data — easy to extend. The
  voice/tense heuristics (passive, past-tense) are grammar-level and
  domain-independent.

- **Lexical-only metadata misses graph context**: A node with no
  distinctive vocabulary but strong edge signals (e.g. a brief heading
  that parents 20 requirement nodes) will have a flat `content_profile`
  in metadata. **Mitigation**: `score_structural` and `score_merge`
  are available as library functions; Issue 85 calls them at layout
  time when the full graph context is available.

- **Code blocks produce noise**: Programming language tokens are not
  natural language — the scorer will produce meaningless results on
  code. **Mitigation**: Code-block nodes should be classified by
  convention (e.g. skip scoring, leave `content_profile` absent),
  not by the lexical scorer.

- **Mixed-content nodes**: Some nodes genuinely carry mixed content (a
  paragraph stating a requirement AND describing the design approach).
  Mixed proportions for mixed content is correct behavior, not a failure.
  High spatial+R mixtures specifically flag content that should be
  decomposed to separate model from observation.

## Design Evolution

This issue underwent a significant architectural pivot during implementation:

1. **Original approach**: exemplar-pole cosine similarity. 20 curated
   passages per content type, TF-IDF centroid vectors, cosine to poles.
   Problem: domain nouns in exemplars ("sensor", "voltage", "connector")
   dominated the signal, making classification domain-dependent.

2. **Pivot**: signal-word + voice/tense heuristics. The key insight
   (documented in [wp-model-ontology] §3.5): content-type signal is in
   the grammar, not the nouns. Passive voice → N, imperative/causal → P,
   past tense → R. Domain-independent by construction.

3. **Stop word refinement**: removed 19 engineering-signal words from
   the stop word list (modals, negation, quantifiers, conditionals).
   These words carry real semantic weight in engineering text and were
   being silently stripped. Benefits both search precision and
   classification accuracy.

4. **Structural channel separated**: edge-topology-based scoring
   (`score_structural`) moved to a library function for Issue 85's
   force-directed layout, rather than blended into the lexical profile
   at parse time. Avoids cascading event complexity in `terminate_stack`.

5. **Signal word curation**: removed `"first"` (ordinal adjective, not
   sequential marker), `"gener"` (ambiguous stem: general/generation vs
   generate), `"propagat"`/`"induc"` (physics-descriptive, not procedural)
   from the P signal list after corpus validation showed dual-firing on
   normative text.

6. **Ontology contribution**: the grammatical signature mapping (§3.5 of
   [wp-model-ontology]) was added during this work — voice/tense → N/S/P/R
   is now a documented ontological property, not just an implementation
   detail.

## Additional fixes discovered during validation

- **PathMap routing bug** (Issue 86 regression): alias Section edges were
  routing to const namespace PathMaps, corrupting path order depth and
  causing cascading re-parses. Fixed by excluding const namespace brefs
  from `candidate_nets` in `process_event_queue`.
- **Codec namespace MISS blacklist**: cache-fetch MISSes on codec/const
  namespace references (C++ `#include` paths) downgraded from WARN to
  DEBUG — eliminates ~1,400 noisy warnings per build.
- **Stop word list refinement**: removed 20 engineering-signal words
  (modals, negation, quantifiers, conditionals, "will") from the search
  stop word list. Benefits both search precision and classification.
- **Repo alias-template**: added `alias-template` to
  `repo/templates/index.md.j2` to resolve 48
  `repo/item-NN` path misses.

## Open Questions

- Should code-block nodes (kind = CodeBlock or similar) be classified as
  `S=1.0` by convention, or skip scoring entirely? Programming language
  tokens are not natural language — the scorer will produce noise.
  Recommend: skip scoring, leave `content_profile` absent.
- The `metadata` map is TOML-serialized. `ContentProfile` values are plain
  floats — no serialization concerns. Older BB exports without profiles
  will simply have no `content_profile` key in metadata (graceful absence).
