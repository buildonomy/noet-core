---
title = "Design Document Index"
authors = "Andrew Lyjak, Claude"
last_updated = "2026-09-03"
status = "Draft"
version = "0.1"
---

# Design Document Index

The design docs are grouped into six topic directories. Each entry below states
the question that document answers, so you can find the right specification
without opening several.

Documents marked **Withdrawn** describe models no longer being built. They are
retained because their analysis or requirements are still cited; check the
`status` field in a document's frontmatter before treating it as current.

## `core/` — the data model and the compiler

What a belief graph *is*, how source files become one, and how it is queried.
Start here.

| Document | Answers |
|---|---|
| [`dag_model.md`](core/dag_model.md) | Why model documentation as a DAG? What are the three edge types (Section, Epistemic, Pragmatic), and what is the subject/verb/referent model? Conceptual introduction — read first. |
| [`architecture.md`](core/architecture.md) | How does the library fit together at a high level, and how does the multi-pass compiler work? Orientation for developers new to noet-core. |
| [`beliefbase_architecture.md`](core/beliefbase_architecture.md) | How is it actually implemented? The authoritative technical spec: identity management (§2.2), codec dispatch (§3.2, §3.6), the event system (§4.3). |
| [`query_model.md`](core/query_model.md) | What is the formal query algebra — traversal, composition, scoring, instruments? The user-facing textual grammar lives in [`docs/query_language.md`](../query_language.md). |
| [`search_and_sharding.md`](core/search_and_sharding.md) | How does full-text search work, and how is a large corpus split into shards so the viewer can scale? |

## `identity/` — what makes a node the same node

How identity survives editing, moving, and reformatting, and how references
name a target stably.

| Document | Answers |
|---|---|
| [`identity_derivation.md`](identity/identity_derivation.md) | Where do identities come from? Minted vs. derived, the reserved namespaces, and the rule that anything surviving a rebuild must be derived. |
| [`content_identity.md`](identity/content_identity.md) | How does a node keep a stable identity when it is moved or reformatted? Defines the identity hash. |
| [`content_versioning.md`](identity/content_versioning.md) | What does "version" mean for a node or a claim, and how is staleness scoped? Defines `_content_hash` (§5.1, §5.1a). |
| [`generational_archive.md`](identity/generational_archive.md) | What does noet retain so "what changed?" is answerable? The stub-shard/blob archive, the move-detection ladder, and the one comparison behind version history, staleness detail, and redlines. |
| [`section_metadata_manifest.md`](identity/section_metadata_manifest.md) | How is per-section metadata tracked and persisted across a compile? |
| [`link_format.md`](identity/link_format.md) | How are cross-document links written and resolved, combining readable paths with stable bref identifiers? |

## `annotation/` — the layers above compiled source

How a corpus accumulates commentary, evidence, and provenance without
modifying the source it annotates.

| Document | Answers |
|---|---|
| [`living_corpus.md`](annotation/living_corpus.md) | What are the three layers (source, compiled, annotation), and how does the annotation loop close? The current model for how a corpus stays alive. |
| [`annotation_channel.md`](annotation/annotation_channel.md) | How does a producer — parse, browser, agent — issue a record? The write-side handle: actor, session, minted `EventId`, two lanes (records vs. diagnostics), the halo of stores. |
| [`overlay_model.md`](annotation/overlay_model.md) | When annotations are projected onto a compiled corpus, what is the resulting object and how is it read? Owner-edge halos, layer composition, and why federation is the same construction. |
| [`collector_model.md`](annotation/collector_model.md) | Where do records live, who may write them, and how do they move between stores? Store topology, admission as a trust boundary, automatic promotion. |
| [`attestation_fabric.md`](annotation/attestation_fabric.md) | How is cross-domain provenance recorded, and how do attestation records project onto graph edges (§12.3)? |
| [`collaboration_overlay.md`](annotation/collaboration_overlay.md) | How can multiple people annotate a *static* generated site, with attested annotations layered over it? |
| [`federated_belief_network.md`](annotation/federated_belief_network.md) | How do separate corpora share annotations, source, and compiled state across a boundary? |

## `procedures/` — operational definitions and as-run records

Turning nodes into executable procedures and capturing what actually happened.
Most of this group is withdrawn; the schema documents remain active.

| Document | Answers |
|---|---|
| [`procedure_schema.md`](procedures/procedure_schema.md) | How is a procedure defined as data on a node? **Active.** |
| [`action_observable_schema.md`](procedures/action_observable_schema.md) | How is an observable action — a step and its expected observation — represented? **Active** as schema; execution integration withdrawn. |
| [`procedure_execution.md`](procedures/procedure_execution.md) | How would runtime tracking and as-run recording work? **Withdrawn**; retained for its requirements. |
| [`redline_system.md`](procedures/redline_system.md) | How are as-run deviations from a written procedure tracked and promoted? **Withdrawn** record types; analysis retained. |
| [`noet_procedures_readme.md`](procedures/noet_procedures_readme.md) | Why observable, auditable, event-driven procedures at all? **Withdrawn** model; positioning argument retained. |

## `codecs/` — reading and writing external formats

How specific input formats are claimed, parsed, and turned into graph
structure. Read `core/beliefbase_architecture.md` §3.2 before adding one.

| Document | Answers |
|---|---|
| [`codec_determinism_contract.md`](codecs/codec_determinism_contract.md) | What must a codec guarantee so that hashes, anchors, and incremental parse work? Six guarantees, none compiler-enforced. **Read before writing or changing a codec.** |
| [`myst_directive_architecture.md`](codecs/myst_directive_architecture.md) | How are MyST block directives (`{network_children}`, `{requirements_table}`) parsed and resolved in the deferred pass? |
| [`network_authoring.md`](codecs/network_authoring.md) | How does an author declare a BeliefNetwork — `index.md`, whitelist/blacklist, subnets? User-facing reference. |
| [`mapping_node_architecture.md`](codecs/mapping_node_architecture.md) | How do mapping nodes own edges, so a node can assert `{maps_to}` claims about others? |
| [`xlsx_codec_schema.md`](codecs/xlsx_codec_schema.md) | What index-tab schema must an XLSX/ODS workbook follow to be ingested? |

## `presentation/` — output surfaces and observability

What the compiler emits for humans, and how the system reports on itself.

| Document | Answers |
|---|---|
| [`interactive_viewer.md`](presentation/interactive_viewer.md) | How does the generated interactive HTML viewer work — navigation, WASM integration, client-side query? |

## Related

- [`figures/`](figures/) — diagrams referenced by the documents above.
- [`../project/DOCUMENTATION_STRATEGY.md`](../project/DOCUMENTATION_STRATEGY.md) — where design docs sit in the wider documentation hierarchy.
