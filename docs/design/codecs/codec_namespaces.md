---
title = "Codec Namespaces and External Resolution"
authors = "Andrew Lyjak, Claude"
last_updated = "2026-09-18"
status = "Draft"
version = "0.1"
dependencies = ["network_authoring.md", "beliefbase_architecture.md"]
---

# Codec Namespaces and External Resolution

## 1. Purpose

A codec namespace is a **synthetic secondary index**: a lookup table of names
that are not filesystem paths, maintained so that cross-references written in a
foreign addressing scheme resolve to the right node.

The motivating case is a C++ `#include`. Source writes
`#include <widget/Widget.h>`, but the header lives at
`src/widget/include/widget/Widget.h`. The include-convention path is not the
filesystem path, and no amount of path arithmetic recovers one from the other
without knowing the build system's include directories. A codec namespace lets
the header *register the name it is cited by*, so citations resolve without
rewriting either side.

This is the inverse of URL aliasing (`network_authoring.md` §8), which lets an
author claim an external URL as a node's address. Both register an alternative
address for a node; URL aliasing is declared in frontmatter by an author, while
a codec namespace is populated programmatically by a codec.

> **Read this before adding a codec that resolves references in a
> non-filesystem addressing scheme** — include paths, package coordinates,
> symbol names, ticket identifiers.

## 2. The three parts

| Part | Who does it | When |
|---|---|---|
| **Register a name** | A codec pushes `(namespace_bid, name)` onto its node's `namespace_paths` | Parse of the *declaring* document |
| **Cite a name** | A codec emits a relation keyed `NodeKey::Path { net: namespace_bref, path: name }` | Parse of the *citing* document |
| **Resolve** | `GraphBuilder::push_relation` looks the key up in the namespace's `PathMap` | Whenever the citing document is parsed |

A namespace BID is derived deterministically from a term:
`Bid::codec_namespace("include")`. The same term always yields the same BID, so
the declaring and citing codecs need share nothing but the string.

`push()` creates the namespace network node lazily on first registration and
calls `register_codec_namespace`, which is what makes `is_codec_namespace`
answer true for it. Nothing needs to declare the namespace up front.

## 3. The resolution timeline, and why it has a sharp edge

A codec namespace is populated *as documents are parsed*, so whether a lookup
succeeds depends on when it happens:

| Pass | A miss means | Correct response |
|---|---|---|
| **First parse** | The declaring document may not have been parsed yet | Re-queue the citing document; try again |
| **Reparse** | Every document has been parsed at least once, so every entry that will ever exist does. The name is declared by nothing | Stop. No later pass can create it |

**The second row is the sharp edge.** Treating a reparse miss as "not yet" makes
the citing document re-queue forever, burning its reparse budget on a reference
that cannot resolve — Issue 97 Bottleneck 3, where this truncated 510 files.
Raising the budget does not help; the work is not converging.

The trap is subtle because the retry signal and the "not yet known bad" signal
are easy to conflate. `process_unresolved_reference` returns `true` to request a
re-queue, and its caller only records a key as permanently unresolved when it
returns `false`. An unconditional `true` therefore *also* prevents the key from
ever being latched, so the optimistic branch can never become pessimistic.

## 4. External resolution

A miss on reparse proves the name is not in the corpus. Often that is correct
source, not an authoring error: C++ including a third-party header is normal.
Reporting it as a broken link misrepresents it, and reporting hundreds of them
drowns the real defects.

A codec may declare that absent names in its namespace denote **out-of-corpus
targets**:

```rust
use noet_core::codec::set_codec_namespace_external;
use noet_core::properties::Bid;

// During codec registration.
set_codec_namespace_external(
    Bid::codec_namespace("include").bref(),
    "c++include::{path}",
);
```

Thereafter a name that misses *on reparse* resolves to an `href_namespace`
external node named by the template, with `{path}` replaced by the missing name.
The citation becomes a first-class outbound edge — queryable as an external
dependency — rather than a warning.

`{path}` is the only substitution. A template may produce a URL
(`"https://docs.example.com/{path}"`) when the namespace has a known
documentation home, or an opaque identifier (`"pkg::{path}"`) when it does not.
`href_namespace` holds both; how the reference renders is the codec's business.

### 4.1 Opt-in, and why

Demotion is per namespace and off by default. For a namespace whose names must
always resolve internally, an absent name is a real defect, and silently
reclassifying it as an external reference converts a bug into a
plausible-looking outbound link — strictly harder to find than the warning it
replaced.

### 4.2 First parse is never demoted

Demotion applies only on reparse. Demoting a first-parse miss would mint an
external node for a name that is about to be registered, permanently shadowing
the real target. This is a correctness constraint, not an optimisation.

### 4.3 Audit the population before opting in

A namespace's absent names are rarely all external. Before enabling demotion,
partition them by a check *independent* of the one that produced the miss — for
include paths, "does a file with this name exist anywhere in the corpus?" — and
confirm each class deserves demotion.

On one C++ corpus that audit found three classes in what looked like a uniform
set of out-of-corpus includes: genuinely external headers, headers **generated
at build time** from in-corpus source (in-corpus provenance; these should
resolve, not demote), and headers that were present and correctly placed but
failed to register because of an unrelated build-file parsing defect. Demoting
the third class would have hidden real defects behind links that looked fine.

### 4.4 Rendering must follow the demotion

A demoted target is an `href_namespace` node whose path is the synthetic
template output. A codec that renders citations must recognise this — check
`BeliefKind::External` on the resolved relation — and not treat it as a
resolved internal target. Building a page link out of
`c++include::widget/Widget.h` yields a dangling href that *looks* resolved.

## 5. API summary

| Function | Purpose |
|---|---|
| `Bid::codec_namespace(term)` | Derive the deterministic namespace BID for a term |
| `register_codec_namespace(bref)` | Called by `push()` on first registration; codecs rarely call this directly |
| `is_codec_namespace(bref)` | Whether a bref names a registered codec namespace |
| `codec_namespace_brefs()` | All registered namespaces (used by the viewer) |
| `set_codec_namespace_external(bref, template)` | Opt the namespace into external resolution (§4) |
| `codec_namespace_external_href(bref, path)` | Apply a namespace's template; `None` if not opted in |

Registration of a *name* is not a function call: push `(namespace_bid, name)`
onto the declaring node's `IRNode::namespace_paths` and `push()` does the rest.

## 6. Related

- `network_authoring.md` §8 — URL aliasing, the author-declared counterpart
- `codec_determinism_contract.md` — a namespace name is part of a codec's
  output and must be as deterministic as the rest of it
- `beliefbase_architecture.md` — `PathMap` and namespace node structure
- Issue 97 Bottleneck 3 — the reparse-budget defect that §3 exists to prevent
