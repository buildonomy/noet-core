---
id = "belief-network-test-1-subnet-2"
title = "Peer Subnet 2 for Nav Tree Testing"
---

## Purpose

This is a peer subnet of [[belief-network-test-1-subnet-1]], both direct
children of the root network.

Having two sibling subnets is the minimal structure needed to trigger the
visited-set interleaving bug that was fixed in `get_nav_tree`: the pre-pass
iterates both subnets and marks them visited, so the build pass (which shared
the same `visited` set under the old code) would find both subnets already
visited and drop their nodes entirely — leaving `subnet2`'s documents
stranded at the root level or absent from the tree.

## Documents

````{network_children}
````
