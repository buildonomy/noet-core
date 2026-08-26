---
title = "Subnet 2 Document"
---

A document that lives inside `subnet2`, a peer of `subnet1` under the root network.

In a correct nav tree this document must appear as a descendant of the `subnet2`
node, which is itself a direct child of the root network.

## Nav Tree Contract

The nav tree bug under test manifested when two or more sibling subnets exist:
the pre-pass iterated all subnets and marked them visited in a shared `BTreeSet`,
so the build pass — which reused that same set — found both subnets already
visited and silently dropped their contents. The result was that documents like
this one either vanished from the tree entirely or were stranded as direct
children of the root network instead of being properly nested under `subnet2`.

This subnet links to [[belief-network-test-1-subnet-1]] to create a cross-subnet
reference that triggers the parallel epoch cache-fetch race.

With the fix in place:
- This document must appear under `subnet2` in the tree, not under root.
- `subnet2` must appear as a child of the root network node.
- `subnet2`'s parent BID must equal the root network's BID.
- `subnet1` must appear as a sibling of `subnet2` at the same depth.
- `subnet1a` must appear as a child of `subnet1`, not of root or `subnet2`.