---
id = "belief-network-test-1-net1-dir1-subnet"
title = "A Subnet Inside a Non-Network Directory"
---

## Purpose

This network exists to exercise the case where a subnet directory is nested
inside a non-network intermediate directory (`net1_dir1/`).

The fast-path stack reconstruction in `try_initialize_stack_from_session_cache`
must correctly build the stack `[root, net1_dir1_subnet]` even though `net1_dir1/`
has no `index.md` and therefore does not appear in any PathMap.  The path key
stored in root's PathMap for this subnet is `"net1_dir1/net1_dir1_subnet"` — a
multi-component string, not a single directory name.

## Documents

````{network_children}
````
