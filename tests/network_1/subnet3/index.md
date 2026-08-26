---
id = "belief-network-test-1-subnet-3"
title = "Peer Subnet 3 for Parallel Epoch BID Stability Testing"
blacklist = ["scratch/**", "scratch.md"]
---

## Purpose

This network exists as a third peer sibling alongside subnet1 and subnet2,
placed in the same parallel epoch batch during `parse_all` to exercise
BID stability across concurrent network parses.

It also exercises the network child filtering feature: files matching the
`blacklist` patterns above are excluded from the BeliefBase entirely.

## Documents

````{network_children}
````
