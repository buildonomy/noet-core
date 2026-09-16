---
title = "noet-core BeliefNetwork example"
id = "belief-network-test-1"
---

A small test directory for BeliefBase codec testing.

Regression coverage: a hand-written link to
[the index-linked target](./index_linked_target_test.md) below, matching the
authoring pattern of `docs/design/codecs/network_authoring.md` §4.2. A peer
document (`index_linked_peer_test.md`) also links to the same target, so this
network is both the target's parent and one of its citers — the shape exercised
by `link_tests::test_index_linked_target_is_path_resolvable_from_peer`.

````{network_children}
````
