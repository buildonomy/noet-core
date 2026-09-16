---
title = "Index-Linked Peer Test"
---

Regression fixture. This document links to
[the target](./index_linked_target_test.md), a document that `index.md` also
cites by a hand-written markdown link.

That coincidence drives two tests in `link_tests`: the link must resolve rather
than being marked permanently unresolved before the target has been parsed even
once (`process_unresolved_reference`'s queued-but-unprocessed branch in
`src/codec/compiler.rs`), and the target must stay PathMap-resolvable so this
link rewrites with a real destination (the weight-union clause in
`BeliefBase::compute_diff` Phase 4).
