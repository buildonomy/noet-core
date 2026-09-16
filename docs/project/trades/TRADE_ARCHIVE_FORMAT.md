---
version = "0.1"
title = "Trade Study: Archive Format for Structural Diff"
authors = "Andrew Lyjak, Claude"
last_updated = "2026-09-13"
status = "Resolved"
---

# Trade Study: Archive Format for Structural Diff

**Resolved.** The specification lives in
[`docs/design/identity/generational_archive.md`](../../design/identity/generational_archive.md);
read that instead.

This study asked what noet must retain, per generation, so that "what changed?"
is answerable as an intelligible edit script rather than as churn. The question
turned out to determine three user-facing features rather than one — version
history, staleness detail, and redlines — so the outcome is a design document
rather than a trade record.

## The options considered

| Option | Outcome |
|---|---|
| **A** — stub keyed on `content_hash` only | **Rejected.** A moved section keeps its `_identity_hash` but routinely moves its `_content_hash`, so this reports every move as remove-plus-add — the exact failure the archive exists to prevent. |
| **B** — stub keyed on `(content_hash, identity_hash, BeliefKindSet)` | **Selected.** Makes an exact move a stub-level set operation with no blob fetch, for ~64 bytes per node. |
| **C** — full retained shards per generation | **Rejected.** ~4× Option B's storage before dedup, with no capability Option B lacks. |
| **Bare blob store** (no retained relations) | **Rejected** before the others: `compute_diff` needs a `BeliefBase`, and re-deriving edges from stored bodies would be a second parse implementation, violating `codec_determinism_contract.md` G1/G2. |

The decisive argument was not storage. Option B makes the *common* case — an
exact move — resolvable from stubs alone, reserving the blob store for the one
question that needs bytes: what text changed.

## What the design document carries forward

The stub format and cost measurements, the three-stage detection ladder (naive
diff → exact `identity_hash` match → TF-IDF over the remainder), the
compiler-versus-builder layering, the redline-as-file-map requirement, the WASM
findings, and the open retention question.
