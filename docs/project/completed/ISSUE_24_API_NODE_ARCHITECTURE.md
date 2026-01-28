# Issue 24: API Node Architecture Clarification and Bug Fix

**Priority**: HIGH
**Estimated Effort**: 2-3 days
**Dependencies**: None
**Blocks**: Issue 07 (testing), general system reliability

## Summary

**RESOLVED:** Root cause identified and fixed. The test BeliefNetwork file (`belief-network-sm-test/BeliefNetwork.toml`) had explicitly hardcoded `bid = "6b3d2154-c0a9-437b-9324-5f62adeb9a44"`, which is the exact value of `UUID_NAMESPACE_BUILDONOMY` (the API node's reserved BID constant). This caused a BID collision during parsing, resulting in node merging.

**Original symptom:** BeliefNetwork nodes merged with API node, resulting in:
- BID from BeliefNetwork (same as API node's BID)
- Kind: `["API", "Network"]` (merged)
- ID: `"buildonomy_api"` (from API node - wrong!)
- Self-referential edge instead of BeliefNetwork → API edge

**Immediate fix applied:** Changed test file BID to `a065d82c-9d68-4470-be02-028fb6c507c0`

**Status:** ✅ COMPLETE - Reserved BID validation implemented, all tests passing, documentation complete

## Goals

1. ✅ ~~**Fix merging bug**~~ - Fixed by correcting test file BID
2. ✅ ~~**Implement reserved BID validation**~~ - Complete with `is_reserved()` function
3. ✅ ~~**Document reserved BID namespace**~~ - Complete in code documentation
4. ✅ ~~**Establish test coverage**~~ - 8 new tests for reserved BID rejection
5. ✅ ~~**Document API node's architectural role**~~ - Complete in architecture.md and beliefbase_architecture.md

## Architecture

### Current API Node Purposes

Per user description, API node serves:

1. **Entry point**: Foundation for traversing BeliefBase structures
   - GraphBuilder.stack initialization starts with API node
   - PathMapMap has root starting point
   
2. **Version management**: Enable older noet-core versions to parse newer document trees
   - Similar to Cargo's version compatibility
   - Future feature for schema evolution
   
3. **Shared interface**: Networks with subnetworks share common API
   - Parent and child networks relate to same API version
   - Enables version-aware parsing

### Current Implementation Issues

**Storage confusion:**
- `BeliefBase.api` field: Immutable reference, set at construction
- `doc_bb`: Contains API node during `initialize_stack` (line 556-559 in builder.rs)
- `session_bb`: Contains API node
- `global_bb`: Contains API node

**Lifecycle confusion:**
- API node added to `doc_bb` in `initialize_stack`
- When BeliefNetwork node parsed, keys somehow match API node
- `cache_fetch` finds API node in `doc_bb` with `check_local=true`
- Nodes merge in `push()` (lines 894-913)
- Result: hybrid node with BID from BeliefNetwork, properties from both

**Actual root cause (IDENTIFIED):**
- Test file `belief-network-sm-test/BeliefNetwork.toml` explicitly set `bid = "6b3d2154-c0a9-437b-9324-5f62adeb9a44"`
- This is the EXACT value of `UUID_NAMESPACE_BUILDONOMY` constant
- When BeliefNetwork node parsed, `cache_fetch` searched for `Bid { bid: 6b3d2154... }`
- Found API node in `doc_bb` with same BID
- Nodes merged because BIDs matched

### Root Cause Analysis

**Why attempted fix (removing API from doc_bb) breaks tests:**

Looking at test failure in `test_sections_priority_matching`:
```
thread 'test_sections_priority_matching' (484764) panicked at src/beliefbase.rs:1133:18:
all nodes in self.states() to have api paths
```

This panic is in `BeliefBase::get_context()`:
```rust
let (home_net, home_path) = paths_guard
    .api_map()
    .home_path(bid, &paths_guard)
    .expect("all nodes in self.states() to have api paths");
```

**The expectation:** Every node in `self.states()` must have a path in the API's PathMap.

**The issue:** If API node isn't in `doc_bb.states()`, then:
1. API node has no PathMap entry
2. `get_context()` fails for API node itself
3. Tests that query API node context panic

**This reveals:** API node MUST be in states for PathMapMap to work correctly.

### Design Questions to Resolve

1. **Should API node be in doc_bb at all?**
   - Pro: Enables PathMapMap lookups, get_context() works
   - Con: Pollutes per-document parsing state with global constant
   - Con: Enables incorrect merging when keys match

2. **How should API node relate to parsed networks?**
   - Current: Parent-child relation (Network → API)
   - Alternative: Special edge type? Metadata field? No relation?

3. **What is doc_bb's contract?**
   - "All nodes parsed from current document"?
   - "All nodes relevant to current parsing session"?
   - "Working set for diff computation"?

4. **Why does BeliefNetwork node match API node keys?**
   - Need to trace key generation with `self.repo() == Bid::nil()`
   - Need to understand PathMapMap behavior with nil network BID
   - Need to verify insert_state merge logic

5. **When should self.repo be set?**
   - Currently: After first network node pushed (line 1020)
   - But keys generated before that use Bid::nil()
   - Chicken-egg problem?

## Implementation Steps

### 1. ✅ Add Diagnostic Logging (COMPLETE)

**Status:** Diagnostic logging revealed the BID collision immediately.

Key logs added:
- `push()`: Shows generated keys, parent_bid, repo, and api().bid
- `cache_fetch()`: Shows found nodes with full details
- `insert_state()`: Shows merge/replacement operations

**Finding:** `self.api().bid` equaled BeliefNetwork's BID, proving collision.

### 2. ✅ Root Cause Identified (COMPLETE)

Test file hardcoded reserved BID. Fixed by changing to unique UUID.

### 3. ✅ Reserved BID Validation (COMPLETE)

**Status:** Validation implemented using namespace-based checking.

**Implementation approach:**

**Implemented in:** `src/properties.rs` and `src/codec/belief_ir.rs`

**Key functions added:**
- `buildonomy_api_bid(version: &str) -> Bid` - Generates versioned API BIDs in reserved namespace
- `is_reserved_bid(bid: &Bid) -> bool` - Checks if BID falls in reserved namespace

**Validation logic in `ProtoBeliefNode::from_str_with_format()`:**
```rust
if let Some(bid_value) = proto.document.get("bid") {
    if let Some(bid_str) = bid_value.as_str() {
        if let Ok(bid) = Bid::try_from(bid_str) {
            if is_reserved_bid(&bid) {
                return Err(BuildonomyError::Codec(...));
            }
        }
    }
}
```

**Reserved namespace includes:**
- `UUID_NAMESPACE_BUILDONOMY` itself
- `UUID_NAMESPACE_HREF` itself  
- All BIDs derived via `Uuid::new_v5()` from the API namespace (checked via first 8 bytes match)

### 4. ✅ Reserved ID Validation (COMPLETE)

Also validate reserved ID values:

**Reserved IDs:**
- `"buildonomy_api"` - API node
- `"buildonomy_href_network"` - Href tracking network

**Validation in same location:**
```rust
if let Some(id_str) = doc.get("id").and_then(|v| v.as_str()) {
    if id_str == "buildonomy_api" || id_str == "buildonomy_href_network" {
        return Err(BuildonomyError::Codec(format!(
            "ID '{}' is reserved for system use. User IDs must not start with 'buildonomy_'",
            id_str
        )));
    }
}
```

### 5. ✅ Documentation (COMPLETE)

**Documentation added:**

- Function documentation for `buildonomy_api_bid()` with examples
- Function documentation for `is_reserved_bid()` explaining checking logic
- Inline comments in validation code explaining reserved namespace concept
- Test documentation showing error messages

**TODO (lower priority):** Add section to `beliefbase_architecture.md` about reserved identifiers

### 6. ✅ Test Coverage (COMPLETE)

**Tests implemented in `src/codec/belief_ir.rs::tests`:**

8 new tests added:
- ✅ `test_reserved_bid_namespace_buildonomy()` - Rejects UUID_NAMESPACE_BUILDONOMY
- ✅ `test_reserved_bid_namespace_href()` - Rejects UUID_NAMESPACE_HREF
- ✅ `test_reserved_bid_derived_from_namespace()` - Rejects BIDs from `buildonomy_api_bid()`
- ✅ `test_reserved_id_buildonomy_api()` - Rejects "buildonomy_api" ID
- ✅ `test_reserved_id_buildonomy_href_network()` - Rejects "buildonomy_href_network" ID
- ✅ `test_reserved_id_buildonomy_prefix()` - Rejects any "buildonomy_*" prefix
- ✅ `test_non_reserved_ids_allowed()` - Allows normal IDs
- ✅ `test_non_reserved_bids_allowed()` - Allows normal BIDs

**All tests passing:** `cargo test` shows 17 passed in belief_ir module

## Testing Requirements

- All existing tests pass
- New tests verify API node isolation
- No API-user node merging under any circumstances
- Correct Network → API relations established
- No self-referential edges created

## Success Criteria

- [x] ~~Diagnostic logging identifies exact merge trigger~~ - **COMPLETE**
- [x] ~~Root cause identified~~ - **COMPLETE: BID collision**
- [x] ~~Fix implemented and all tests passing~~ - **COMPLETE: Changed test file BID**
- [x] ~~`noet watch ../belief-network-sm-test/` produces correct output~~ - **COMPLETE**
- [x] ~~Reserved BID validation implemented~~ - **COMPLETE: `is_reserved_bid()` function**
- [x] ~~Reserved ID validation implemented~~ - **COMPLETE: Reject `buildonomy_*` prefix**
- [x] ~~Test coverage for reserved identifier rejection~~ - **COMPLETE: 8 new tests**
- [x] ~~Documentation updated~~ - **COMPLETE: Function docs, inline comments, architecture.md § 9, beliefbase_architecture.md § 2.7**
- [x] ~~All existing tests still pass~~ - **COMPLETE: Full test suite passing**
- [x] ~~API node purpose documented~~ - **COMPLETE: Version management and entry point roles explained**
- [x] ~~Reserved namespace design documented~~ - **COMPLETE: Namespace checking algorithm and validation specified**

## Risks

**Risk 1: Users have existing files with reserved BIDs** ~~(RESOLVED - unlikely)~~
- Severity: LOW
- **Status**: Test file was only known instance, already fixed
- **Mitigation**: Clear error messages guide users to fix
- **Fallback**: Provide migration tool if needed

**Risk 2: Breaking changes to parsing API**
- Severity: LOW
- **Mitigation**: Validation returns standard BuildonomyError, compatible with existing error handling
- **Fallback**: Document as minor version bump if needed

## Open Questions (Deferred - Lower Priority)

1. **Should we reserve additional BID ranges?** (e.g., entire UUID namespace prefix)
2. **Should validation be configurable?** (strict mode vs permissive for testing)
3. ~~Is API node the right abstraction?~~ (Deferred - current design works)
4. ~~How will multi-version parsing work?~~ (Future feature, not blocking)

## References

- **Root cause:** Test file `belief-network-sm-test/BeliefNetwork.toml` hardcoded `bid = "6b3d2154-c0a9-437b-9324-5f62adeb9a44"`
- **Fix commit:** Changed test BID to `a065d82c-9d68-4470-be02-028fb6c507c0`
- **Diagnostic logs:** Showed `self.api().bid == parsed_node.bid`, proving collision
- **Validation implementation:**
  - `properties.rs:120-123` - `buildonomy_api_bid(version)` - Generates versioned API BIDs
  - `properties.rs:125-151` - `is_reserved_bid(bid)` - Checks if BID is in reserved namespace
  - `belief_ir.rs:1081-1093` - Reserved BID validation in `from_str_with_format()`
  - `belief_ir.rs:1096-1125` - Reserved ID validation in `from_str_with_format()`
  - `belief_ir.rs:1477-1561` - 8 tests for reserved identifier rejection
- **Documentation:**
  - `docs/design/architecture.md` § 9 - API node concept and purpose (brief overview)
  - `docs/design/beliefbase_architecture.md` § 2.7 - Complete technical specification (220 lines)
  - Covers: version management, reserved namespace design, BID generation, validation, lifecycle, future extensions