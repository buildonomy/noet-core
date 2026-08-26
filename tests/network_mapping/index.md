---
title = "Mapping Test Network"
id = "mapping-test-network"
---

A dedicated test network for {maps_to} directive integration tests.

Kept separate from `network_1` so that cross-document mapping edges with
ephemeral section BIDs do not interfere with `bid_tests.rs` idempotency checks.

````{network_children}
````
