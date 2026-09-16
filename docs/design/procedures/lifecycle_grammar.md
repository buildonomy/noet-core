---
title = "The Lifecycle Grammar: Exit Predicates, Outcomes, and Effects"
authors = "Andrew Lyjak, Claude"
last_updated = "2026-09-14"
status = "Stub — specification pending Issue 17"
version = "0.1"
dependencies = ["procedure_model.md", "query_model.md"]
---

# The Lifecycle Grammar

> [!IMPORTANT]
> **This document is a stub. The grammar it will specify is not built.**
>
> [`procedure_model.md`](./procedure_model.md) establishes that a procedure's
> state is a marking derived from records. This document will specify the
> notation that declares *when* a step is marked and *what* a mark does — as
> registered MyST directives over ordinary markdown.
>
> **Issue 17 steps 1, 2, and 2a build it.** What follows records the decisions
> already made, so that implementation resolves the remaining detail rather than
> reopening settled questions. Nothing here is a specification to implement
> against; §4 lists what a reader may rely on today.

## 1. What This Document Will Own

| Concern | Detail |
|---|---|
| `{exit}` | the predicate that marks a step: `all` / `any` / `ordered` / `n-of` |
| `{outcome}` | the discriminated exits a step admits, and the effect of each |
| `:over:` | the queryset a predicate ranges over, defaulting to containment children |
| `:clears:` | the one effect other than marking — how a cycle is expressed |
| Marking semantics | what the fold does with a record that discharges a step |
| Procedural record kinds | `redline` and `ask`, and their payload schemas |

## 2. Settled Decisions

These constrain the specification and are not open at implementation time.

**No codec and no file format.** The grammar is a set of directives over
markdown. A step is already a node with a stable BID
([`procedure_model.md`](./procedure_model.md) §2), so a second parser producing
the same node shape from a structured file body would be a second authoring
surface for one concept.

**No new record primitives.** A procedural record is an ordinary annotation
carrying a registered `record_kind` and a payload schema — the same mechanism
`{todo}` and `{reviewed}` use. Nothing is added to the event type.

**Selection comes from the query model; combination comes from nesting.** The
`:over:` option is a query string. The predicate leaf therefore gets **no
`and` / `or` / `not`** — `all`/`any` over a nested set already *is* the boolean
algebra. This is the structural reason the `:over:` slot cannot grow into a
general condition language, and it is the mitigation for the main risk the
grammar carries.

**A cycle is an outcome that clears marks.** Not a back-edge, not a transition
construct, not a second grammar ([`procedure_model.md`](./procedure_model.md)
§4.2).

**An outcome is a derived enumeration**, determined by an actor assertion, a
derived condition, or a cited run's state — never read off a single payload
field ([`procedure_model.md`](./procedure_model.md) §4.4). The grammar must
therefore let a step *declare which outcomes it admits* without fixing where
each one's determinant comes from.

**Open enums, loud on the unknown.** An unrecognized combinator, outcome, or
`record_kind` validates structurally and fails loudly at fold time. A record
whose lifecycle is unavailable stays readable and simply has no derived state
(`attestation_fabric.md` §6.2).

**Cross-run guards need no syntax.** Issue 105's fold derives runs in
`caused_by`-topological order and puts derived state in the run node's payload,
and `resolve_property_path` already reaches payload from the query grammar. A
guard is therefore an `:over:` query with a payload predicate, not a new
construct.

## 3. The Shape, Illustrated

Indicative, not normative — the option spellings are exactly what Issue 17
step 1 settles.

````markdown
## Change plan lifecycle {#plan-lifecycle}

```{exit} ordered
```

- {#drafted} Proposed text is complete.
- {#placed} Target section is pinned.
- {#packaged} Rendered as a change request.
  ```{exit} all
  :over: constrained_by(1)
  ```
- {#submitted} Handed to the process owner.
  ```{outcome} rejected
  :clears: #drafted
  ```
````

`#packaged` does not complete until every node it declares itself constrained by
is discharged — a blocking relation expressed as a queryset rather than as a
status value. `rejected` on `#submitted` clears `#drafted`'s mark, which is the
whole of the rejection path.

## 4. What a Reader May Rely On Today

Nothing in this document is implemented. What *is* implemented, and what the
grammar will be built on:

- A step is a node with a stable BID (Issue 91B)
- Fenced directives with `:key:` options parse (`src/codec/myst.rs:659`)
- Query shorthands for traversal parse (`src/query/parser.rs:414`)
- `payload.x > n` parses and resolves (`src/query/spec.rs:580`)

## 5. Open Items for Implementation

1. **The composed spelling.** A traversal composed with a payload predicate must
   be confirmed against `query_model.md` §9.5. If it is not expressible, that is
   a gap in the query model, not a gap to fill here.
2. **Over-graph cycles.** Because `:over:` is an arbitrary query, the graph of
   `over:` relations can cycle. Exit evaluation and `clears` propagation both
   walk it and need a visited set; a cycle is a template lint. This does **not**
   apply to the record fold, which is a linear pass and cannot diverge.
3. **Where a determinant is declared.** §2 fixes that an outcome is derived from
   one of three sources. Whether the *source* is declared at the outcome site or
   inferred from the record kind is undecided.
4. **Cross-record value reads.** A captured value lives in the payload of the
   record that captured it under the key `stores_in_variable` names
   ([`observation_model.md`](./observation_model.md)). How a later step's
   predicate resolves that name across records in the same run is unspecified.
5. **Whether `Bid::codec_namespace` can carry schema versions.** An open
   combinator enum makes the type set a versioning surface; whether the
   namespace mechanism is the right carrier is Issue 32's broader question,
   narrowed here.

## 6. References

- [`procedure_model.md`](./procedure_model.md) — what a procedure is and where state comes from
- [`observation_model.md`](./observation_model.md) — what discharges a step
- [`deviation_model.md`](./deviation_model.md) — the deviation vocabulary a fold produces
- [`../annotation/redline_model.md`](../annotation/redline_model.md) — the general `redline` kind
- [`../core/query_model.md`](../core/query_model.md) §9.5 — the language `:over:` is written in
- [`../codecs/myst_directive_architecture.md`](../codecs/myst_directive_architecture.md) §3, §8 — the directive registry and its extension point
- [`../annotation/living_corpus.md`](../annotation/living_corpus.md) §4 — why a lifecycle is a referenced document rather than an embedded block
- `ISSUE_17_NOET_PROCEDURES_EXTRACTION.md` — builds this
- `ISSUE_105_RECORD_STORE_AND_FOLD.md` — consumes the exit-predicate semantics
