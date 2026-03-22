---
id = "belief-network-test-1-subnet-1a"
title = "A Nested Sub-Network for Nav Tree Testing"
---

## Purpose

This network exists to exercise the three-level network hierarchy:
root → [[belief-network-test-1-subnet-1]] → `subnet1a`.

The nav tree bug under test caused highly nested subnetwork documents to be
interleaved into the root nav tree rather than nested under their proper parent
stem. With the fix in place, nodes from this network must appear as children of
`subnet1` in the navigation tree, not as children of the root network.

## Documents

````{network_children}
````
