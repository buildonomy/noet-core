---
title = "Net1 Dir1 Subnet Document"
---

A document that lives inside `net1_dir1_subnet`, which is itself nested inside
the non-network directory `net1_dir1/`.

This file exercises the fix in `try_initialize_stack_from_session_cache`: the
fast-path stack reconstruction must produce `[root, net1_dir1_subnet]` for this
document, even though the intermediate directory `net1_dir1/` has no `index.md`
and is therefore invisible to the PathMap system.

The path key stored in root's PathMap for the parent subnet is
`"net1_dir1/net1_dir1_subnet"` — a multi-component string.  The fix uses
`proto_index` to identify which ancestor directories are networks (skipping
`net1_dir1/`) and resolves each hop through the PathMap of the containing
network rather than splitting the path string by `/`.

## Details

This file has no cross-network links by design.  Its sole purpose is to give
`net1_dir1_subnet` a visible non-index document so the nav tree contains at
least one non-network leaf node under this subnet.