---
title = "Index-Linked Target Test"
---

This document is cited by a hand-written link in `index.md` (see
`network_children`'s neighbouring list item), and is also linked to from
`index_linked_peer_test.md`. Regression fixture: a link to a target that
`index.md` also cites must resolve identically to any other internal link,
regardless of parse order.

Being cited by its own parent network means this document's `Section` edge to
that network and the citation's `Epistemic` edge share one `(source, sink)`
pair — the case the weight-union clause in `BeliefBase::compute_diff` Phase 4
exists to preserve.

## Target Section {#index-linked-target}

Content that a peer document links to.
