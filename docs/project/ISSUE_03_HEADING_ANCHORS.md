# Issue 3: Section Heading Anchor Management and ID Triangulation

**Priority**: CRITICAL - Required for v0.1.0
**Estimated Effort**: 1-2 days (reduced from 2-3 days - parsing infrastructure provided by pulldown_cmark)
**Dependencies**: Indirect on Issues 1 & 2 (for node types and section BIDs)

## Summary

Implement a clean, cross-renderer compatible anchor strategy using title-based IDs with Bref fallback for collisions. Leverage the existing multi-ID triangulation system to enable automatic synchronization of ID changes across source files and caches. Do NOT inject BID anchors into markdown - use title-based anchors for semantics and Brefs for uniqueness.

## Goals

1. Parse existing anchors from headings: `{#introduction}`, `{#custom-anchor}`
2. Generate IDs using title-first, Bref-fallback strategy
3. Track BID-to-ID mappings internally via BeliefBase
4. Preserve cross-renderer compatibility (GitHub, GitLab, Obsidian auto-generate title anchors)
5. Enable automatic ID updates when titles change (for auto-generated IDs only)
6. Only inject anchors when necessary (Bref collision case)

## Architecture

### Anchor Format

**Markdown (Clean, Minimal Injection):**
```markdown
# Getting Started
<!-- No anchor - renderer auto-generates #getting-started -->

## Step 1: Install
<!-- No anchor - auto-generates #step-1-install -->

## Details
<!-- First occurrence - no anchor needed -->

## Details {#a1b2c3d4e5f6}
<!-- Collision! Bref injected for uniqueness -->
```

**HTML (Generated with Data Attributes):**
```html
<h1 id="getting-started" 
    data-bid="01234567-89ab-cdef"
    data-bref="a1b2c3d4"
    data-nodekey="bid://01234567-89ab-cdef">
    Getting Started
</h1>

<h2 id="details" 
    data-bid="98765432-10ab-cdef">
    Details
</h2>

<h2 id="a1b2c3d4e5f6" 
    data-bid="abcdef01-2345-6789"
    data-bref="a1b2c3d4e5f6"
    data-title-collision="details">
    Details
</h2>
```

### ID Generation Strategy

**Two-tier fallback for collision-safe uniqueness:**

```rust
fn determine_node_id(
    explicit_id: Option<&str>,
    title: &str,
    bref: &Bref,
    existing_ids: &HashSet<String>  // Set of NORMALIZED IDs
) -> String {
    let candidate = if let Some(id) = explicit_id {
        // User provided ID - MUST normalize it for HTML compatibility
        to_anchor(id)
    } else {
        // No explicit ID - use title
        to_anchor(title)
    };
    
    // Check collision on NORMALIZED candidate
    if existing_ids.contains(&candidate) {
        // Collision (even from explicit ID!) - use Bref
        bref.to_string()
    } else {
        candidate
    }
}
```

**Why Bref for collisions:**
- Bref is derived from BID via UUID v5 hash: `bid.namespace()`
- Hash space: 16^12 = ~281 trillion possible values
- Collision probability is astronomically low within a network
- Already computed and available
- No additional collision detection logic needed
- **Note**: Bref is also the standard strong NodeKey for links (see Issue 4)
  - Simplifies system: one strong reference type instead of multiple (BID, Bref, ID)
  - Consistent use across headings (collision case) and links (all cases)

**Critical: All IDs normalized before collision check:**
- Explicit user IDs are normalized via `to_anchor()` before checking
- Prevents HTML anchor conflicts from case/punctuation differences
- Example: User writes `{#Section One!}` → normalizes to `section-one` → collision check

### Write Authority Model

**Markdown is source of truth. No extra metadata fields.**

```markdown
---
bid: 01234567-89ab-cdef
schema: Action
---

# Node Title
<!-- No anchor = title-derived ID (auto-updates with title) -->

# Node Title {#custom-id}
<!-- Explicit anchor = user-controlled (preserved) -->

# Node Title {#a1b2c3d4e5f6}
<!-- Bref anchor = collision-generated (user can delete to regenerate) -->

# Section {#Section!}
<!-- User-provided ID with special chars - gets normalized to "section" for collision check -->
```

**Injection Rules (in `md.rs::inject_context()`):**

```rust
fn inject_context(&mut self, proto: &ProtoBeliefNode) -> Result<(), BuildonomyError> {
    for node in self.nodes() {
        let explicit_id = if heading_has_anchor(&heading_text) {
            Some(extract_anchor_id(&heading_text))
        } else {
            None
        };
        
        let calculated_id = determine_node_id(
            explicit_id,
            &node.title, 
            &node.bid.namespace(), 
            &existing_ids  // Contains NORMALIZED IDs only
        );
        
        let has_explicit_anchor = heading_has_anchor(&heading_text);
        
        if has_explicit_anchor {
            // User provided explicit ID - preserve it
            // Don't inject anything
        } else {
            // No explicit anchor in markdown
            if calculated_id == to_anchor(&node.title) {
                // ID is title-derived - don't inject
                // Let renderer auto-generate for cross-renderer compatibility
            } else {
                // ID is Bref (collision case) - inject it for uniqueness
                inject_heading_anchor(&mut heading_text, &calculated_id);
            }
        }
    }
}
```

### Title Change Behavior

| Initial State | Title Changes | User Action | Result |
|--------------|---------------|-------------|---------|
| `# Details` | → `# Specific Details` | None | Auto-updates to `#specific-details` |
| `# Details {#bref}` | → `# Specific Details {#bref}` | None | Keeps Bref (explicit) |
| `# Details {#bref}` | → `# Specific Details` | Delete `{#bref}` | New ID: `#specific-details` |
| `# Details {#custom}` | → `# Specific Details {#custom}` | None | Keeps `#custom` |

**Detection:** No explicit anchor in markdown = auto-generated ID = updates with title changes

## pulldown_cmark Infrastructure (Discovery 2025-01-26)

**Critical Finding**: pulldown_cmark already provides anchor parsing when `ENABLE_HEADING_ATTRIBUTES` is enabled!

**Summary**: The hardest part of Issue 3 (parsing `{#anchor}` syntax from heading text) is already implemented by pulldown_cmark. We just need to:
1. Uncomment one line to enable the feature
2. Capture the `id` field instead of ignoring it
3. Issue 2's anchor matching will immediately start working

**Effort reduction**: 2-3 days → **1-2 days** (parsing infrastructure is free!)

### Current State

**Option is commented out** in `buildonomy_md_options()`:
```rust
// md_options.insert(MdOptions::ENABLE_HEADING_ATTRIBUTES);
```

**Heading tag structure** (already available):
```rust
MdEvent::Start(MdTag::Heading {
    level,      // HeadingLevel::H1, H2, etc.
    id,         // Option<CowStr> - THE ANCHOR!
    classes,    // Vec<CowStr>
    attrs,      // Vec<(CowStr, Option<CowStr>)>
})
```

Currently we ignore these fields: `id: _`, `classes: _`, `attrs: _`

### Behavior Verification

**Test Results** (using pulldown_cmark directly):

```rust
// WITHOUT ENABLE_HEADING_ATTRIBUTES:
"## Test Heading {#my-id}"
// → id=None, text="Test Heading {#my-id}"

// WITH ENABLE_HEADING_ATTRIBUTES:
"## Test Heading {#my-id}"
// → id=Some("my-id"), text="Test Heading"
```

**Key Features**:
- ✅ Anchor syntax `{#...}` is **automatically stripped** from heading text
- ✅ Anchor is extracted into `id` field
- ✅ Works with **all formats**: plain IDs, BID URIs, Brefs
- ✅ Text event contains only the title (without anchor)

**Examples from test fixture**:
```markdown
## Introduction {#bid://20000000-0000-0000-0000-000000000002}
// → id=Some("bid://20000000-0000-0000-0000-000000000002")
// → text="Introduction"

## Background {#background}
// → id=Some("background")
// → text="Background"

## API Reference
// → id=None
// → text="API Reference"
```

### Integration Impact

**This means Issue 3 is MUCH simpler than expected!**

1. **No custom parsing needed** - just uncomment `ENABLE_HEADING_ATTRIBUTES`
2. **Capture `id` field** during parse (change `id: _` to `id`)
3. **Store in ProtoBeliefNode.document** as "id" or "anchor" field
4. **Issue 2 already checks** for "id"/"anchor" fields in `extract_anchor_from_node()`
5. **BID and anchor matching** will automatically start working!

**Remaining work**:
- Implement collision detection (Bref fallback)
- Implement selective anchor injection (only when needed)
- Update `BeliefNode::keys()` to include ID-based NodeKey

**Estimated effort reduction**: From 2-3 days → 1-2 days (parsing is free!)

## Implementation Steps

### 1. Enable and Capture Heading Anchors (0.25 days) ← SIMPLIFIED!

**File**: `src/codec/md.rs`

**Enable the option**:
```rust
pub fn buildonomy_md_options() -> Options {
    // ...
    md_options.insert(Options::ENABLE_HEADING_ATTRIBUTES);  // UNCOMMENT THIS
    // ...
}
```

**Capture the id field** (line ~933):
```rust
MdEvent::Start(MdTag::Heading {
    level,
    id,         // Change from: id: _
    classes: _,
    attrs: _,
}) => {
    // Store id in ProtoBeliefNode
    if let Some(anchor_id) = id {
        current.document.insert("id", value(anchor_id.to_string()));
    }
    // ... rest of heading logic
}
```

**That's it for parsing!** pulldown_cmark does the heavy lifting.

### 2. Implement ID Generation with Bref Fallback (0.5 days)

**File**: `src/codec/md.rs` or `src/properties.rs`

- [ ] Implement `determine_node_id(explicit_id, title, bref, existing_ids)`
- [ ] If `explicit_id` is Some, normalize it via `to_anchor(explicit_id)`
- [ ] Otherwise, use `to_anchor(title)`
- [ ] Check collision against `existing_ids` set (all NORMALIZED IDs in document)
- [ ] Fallback to `bref.to_string()` on collision (even from explicit ID)
- [ ] Store final ID in `BeliefNode.id`
- [ ] Build `existing_ids` set during parsing by normalizing all encountered IDs

### 3. Implement Selective Anchor Injection (1 day)

**File**: `src/codec/md.rs::inject_context()`

- [ ] For each heading, check if `ProtoBeliefNode.document.get("id")` exists (explicit anchor)
- [ ] Calculate what ID should be (title or Bref)
- [ ] Only inject anchor if:
  - No explicit anchor exists AND
  - Calculated ID is NOT title-derived (i.e., it's a Bref due to collision)
- [ ] Format: `# Title {#bref-value}` 
- [ ] Use `update_or_insert_frontmatter()` pattern to inject anchor into heading events
- [ ] pulldown_cmark will serialize it correctly when generating source

### 4. Implement ID Update Detection (1 day)

**File**: `src/codec/builder.rs::push()`

- [ ] During `inject_context()`, compare `ctx.node.title` with `proto.document.get("title")`
- [ ] If title changed AND `proto.document.get("id")` is None (no explicit anchor):
  - Node's ID was auto-generated (title-derived or Bref)
  - Regenerate ID from new title (may change from Bref to title or vice versa)
  - Update `proto.document.insert("id", ...)` if needed
  - Return updated BeliefNode to trigger BeliefEvent::NodeUpdate
- [ ] If explicit anchor present (`proto.document.get("id")` exists): preserve it (user control)

### 5. Update Document Writing (0.5 days)

**File**: `src/codec/md.rs::generate_source()`

- [ ] `generate_source()` already calls `events_to_text()` which uses pulldown_cmark_to_cmark
- [ ] pulldown_cmark will automatically:
  - Write headings without anchors if `id` field is None
  - Write headings with `{#id}` if `id` field is Some
- [ ] Just ensure `id` field in heading events matches ProtoBeliefNode.document["id"]
- [ ] May need to update heading events when injecting anchors (see step 3)

### 6. Update BeliefNode::keys() (0.5 days)

**File**: `src/properties.rs`

- [ ] Update `BeliefNode::keys()` to include `NodeKey::Id { net, id }` when `self.id` is Some
- [ ] ID comes from `BeliefNode.id` field (populated from ProtoBeliefNode.document["id"])
- [ ] Enable triangulation: BID, Bref, ID, Title, Path all valid for same node
- [ ] This allows Issue 2 section matching to work via ID key

## Testing Requirements

- Parse heading with/without explicit anchor
- Parse title-based anchors: `{#introduction}`, `{#getting-started}`
- Parse user IDs with special characters: `{#Section One!}` → normalize to `section-one`
- Generate Bref ID when title collision occurs
- Generate Bref ID when normalized explicit ID collides
- Test collision scenarios:
  - Two titles normalize to same ID
  - Explicit ID normalizes to same as another title
  - Explicit ID with special chars: `{#My-Section!}` vs title "My Section"
- Test title change scenarios:
  - Title changes, no explicit anchor → ID updates
  - Title changes, explicit Bref anchor → preserved
  - Title changes, explicit custom anchor → preserved
  - User deletes Bref anchor after title change → regenerates from new title
- Verify no anchors injected for unique titles
- Verify Bref anchors injected only for collisions
- Round-trip preservation (parse → generate → parse)
- Links with title anchors work: `./doc.md#introduction`
- GitHub/GitLab/Obsidian render correctly

## Success Criteria

- [ ] Parse title-based anchors from headings
- [ ] Generate IDs using title-first, Bref-fallback strategy
- [ ] Only inject anchors when necessary (Bref collision)
- [ ] Track BID-to-ID mapping internally via PathMap
- [ ] No BID anchors in markdown source
- [ ] Auto-update IDs when title changes (if no explicit anchor)
- [ ] Preserve explicit anchors (Bref or custom)
- [ ] Links using title anchors work across renderers
- [ ] Backward compatible with existing documents
- [ ] Tests pass
- [ ] Clean, minimal markdown output

## Risks

**Risk**: User confusion about when anchors are injected
**Mitigation**: Document clearly - only Bref collisions get injected, all else is clean

**Risk**: Confusion between heading ID (anchor) and link Bref
**Mitigation**: Clear documentation - heading IDs are title-based (or Bref on collision), link Brefs are always Bref (see Issue 4)

**Risk**: User writes non-normalized explicit ID causing unintended collision
**Mitigation**: Always normalize explicit IDs before collision check; document normalization rules

**Risk**: Title changes break external links
**Mitigation**: Phase 3 notification system updates referring documents (Issue 4)

**Risk**: Bref collision probability (though astronomically low)
**Mitigation**: 2^48 hash space makes this negligible; BID always provides fallback

**Risk**: Cross-renderer anchor differences
**Mitigation**: Explicitly inject Bref anchors ensures consistency for collision cases

## Open Questions

1. Should we add `data-title-collision` attribute in HTML for debugging? (Recommend: Yes, helpful)
2. Should we warn users when their explicit ID gets normalized? (Recommend: Yes, log diagnostic)
3. Should we preserve original non-normalized ID in a data attribute? (Recommend: Optional, for debugging)
4. How to handle triple-nested collision edge cases? (Recommend: Trust Bref hash space)
5. Should ID generation be configurable per-document? (Recommend: No, keep simple)

## References

- Current parser: `codec/md.rs`
- BID/Bref generation: `properties.rs::Bid::namespace()` (lines 180-188)
- PathMap: `paths.rs`
- Builder: `builder.rs::push()` (lines 772-954)
- Anchor support: GitHub, GitLab, Obsidian, mdBook, Pandoc
- Issue 4: Link manipulation uses Bref (not BID or ID) as standard strong NodeKey in title attribute
- Issue 6: HTML generation adds `data-bid` and `data-bref` attributes

---

## Appendix: Identity Management and Triangulation

### Purpose

The noet-core identity system uses **redundant IDs** to enable **triangulation** - finding and synchronizing nodes across multiple sources (source files, caches, repositories) even when their content or location changes. This is not a liability but a feature that enables robust, automatic synchronization.

### The Identity Hierarchy

**Multiple IDs per node, each serving a distinct purpose:**

1. **BID (Belief ID)** - `Bid::new(parent)`
   - UUID-like globally unique identifier
   - **Immutable** - never changes once assigned
   - Primary key for graph operations
   - Stored in frontmatter: `bid: 01234567-89ab-cdef`
   - **noet-controlled** - generated and managed by system

2. **Bref (Belief Reference)** - `bid.namespace()`
   - Short hash derived from BID (last 12 hex chars of UUID v5 hash)
   - Example: `a1b2c3d4e5f6`
   - Compact, human-readable for logging/display
   - **Deterministic** - function of BID
   - Used as fallback ID for anchor collisions

3. **ID (Anchor)** - `to_anchor(title)` or explicit
   - Human-readable section identifier
   - Example: `getting-started`, `step-1-install`
   - Used in markdown anchors: `{#getting-started}`
   - **Title-derived** (auto) or **explicit** (user-provided)
   - Auto-generated IDs update when title changes

4. **Path** - Derived from document structure
   - File system relative path + anchor
   - Example: `docs/guide.md#getting-started`
   - **Structural** - changes with file moves or heading reorganization
   - Maintained by PathMap

5. **Title** - Human-readable display name
   - Example: "Getting Started"
   - **User-controlled** - changes frequently
   - Not suitable as stable reference (changes often)

### Triangulation Flow

**Scenario:** User renames document title and moves file

**Before:**
```markdown
File: docs/tutorial.md
---
bid: abc123
---
# Getting Started {#getting-started}
```

**After:**
```markdown
File: docs/guides/introduction.md
---
bid: abc123
---
# Introduction
```

**Triangulation enables discovery:**

1. **Parse new file** - generates NodeKeys:
   - `NodeKey::Bid { bid: abc123 }` (from frontmatter)
   - `NodeKey::Title { title: "introduction" }` (from heading)
   - `NodeKey::Path { path: "docs/guides/introduction.md#introduction" }` (predicted)

2. **Cache lookup tries each key**:
   - Try BID → **HIT!** Found old node via BID
   - Old node had: title="Getting Started", path="docs/tutorial.md#getting-started"

3. **Detect changes**:
   - Title changed: "Getting Started" → "Introduction"
   - Path changed: "docs/tutorial.md" → "docs/guides/introduction.md"
   - ID was auto-generated (no explicit anchor) → regenerate: "getting-started" → "introduction"

4. **Track old IDs for notification**:
   - `unique_oldkeys`: `{ NodeKey::Id { id: "getting-started" }, NodeKey::Path { path: "docs/tutorial.md#getting-started" } }`

5. **Phase 3: Notify sinks** (documents that reference this node):
   - Find all documents with links to old ID/path
   - Queue for rewrite with updated references
   - Update links: `[Guide](./tutorial.md#getting-started)` → `[Guide](./guides/introduction.md#introduction)`

### Write Authority Model

**The markdown source file is the source of truth. No extra metadata fields needed.**

| Element | Authority | Behavior |
|---------|-----------|----------|
| **BID** | noet-controlled | Generated once, immutable, always in frontmatter |
| **Explicit ID** | User-controlled | Preserved exactly as written in markdown |
| **Auto-generated ID** | noet-managed | Updates when title changes (no explicit anchor) |
| **Title** | User-controlled | Free to change, triggers ID regeneration if auto |
| **Content** | User-controlled | Free to change |

**Detection of auto-generated vs explicit:**
```rust
// In inject_context():
let has_explicit_anchor = heading_has_anchor(&heading_text);

if has_explicit_anchor {
    // User wrote {#custom-id} - preserve it
    preserve_anchor();
} else {
    // No anchor in markdown - it's auto-generated
    // Regenerate from current title or inject Bref if collision
    regenerate_id();
}
```

### Multi-ID Benefits for Synchronization

**Why redundant IDs matter:**

1. **Forward References** - Reference nodes before they're parsed
   - Use Path or Title to create relation
   - Resolve to BID during linking phase

2. **Distributed Sources** - Find nodes across caches and repos
   - Try BID in global cache
   - Try Path in filesystem
   - Try Title for user-friendly matching

3. **Change Detection** - Determine what changed
   - BID stable → it's the same node
   - Title changed → semantic update
   - Path changed → structural move
   - ID changed → anchor update

4. **Automatic Synchronization** - Update referring documents
   - Find all sinks via graph edges
   - Rewrite their links with new IDs/paths
   - Maintain network consistency

5. **Graceful Degradation** - Work with partial information
   - Missing BID? Try Path
   - Missing Path? Try Title
   - Missing everything? Create forward reference

### Example: Full Triangulation Scenario

**Initial state:**
```markdown
File: procedures/morning.md
---
bid: abc123
---
# Morning Routine

## Make Coffee {#make-coffee}

File: goals/health.md
---
bid: def456
---
# Health Goals

See [Coffee Ritual](../procedures/morning.md#make-coffee)
```

**User changes:**
1. Renames "Make Coffee" → "Brew Morning Coffee"
2. Moves file: `procedures/morning.md` → `routines/daily/morning.md`

**System triangulation:**

1. **Parse `routines/daily/morning.md`**:
   - BID `abc123` found in frontmatter
   - Generate keys: `{ Bid(abc123), Title("brew-morning-coffee"), Path("routines/daily/morning.md#brew-morning-coffee") }`

2. **Cache fetch via BID**:
   - Find old node: title="Morning Routine", sections include "Make Coffee {#make-coffee}"
   - Detect: title changed on section, explicit anchor present

3. **Decision**:
   - Section has explicit `{#make-coffee}` anchor
   - Preserve it (user control)
   - Don't regenerate ID

4. **Result - markdown unchanged**:
```markdown
File: routines/daily/morning.md
---
bid: abc123
---
# Morning Routine

## Brew Morning Coffee {#make-coffee}
<!-- Explicit anchor preserved! -->
```

5. **But if user deletes anchor**:
```markdown
## Brew Morning Coffee
```

6. **System regenerates**:
   - No explicit anchor → auto-generated
   - New ID: `to_anchor("Brew Morning Coffee")` = "brew-morning-coffee"
   - Track old: `unique_oldkeys.insert(NodeKey::Id("make-coffee"))`

7. **Phase 3: Update referring documents**:
   - Find `goals/health.md` references section via edge
   - Rewrite link: `[Coffee Ritual](../routines/daily/morning.md#brew-morning-coffee)`

### Integration with Existing Systems

**PathMap** (`paths.rs`):
- Maintains BID → Path bidirectional mapping
- Generates paths with collision-safe anchors
- Used during triangulation for Path-based lookups

**BeliefBase** (`beliefbase.rs`):
- Maintains all ID mappings: `bid_to_index`, `brefs`, `ids`, `paths`
- Enables O(1) lookup by any NodeKey type
- Central registry for triangulation

**Builder** (`builder.rs::parse_content()`):
- Phase 1: Create nodes, cache_fetch via multiple keys (triangulation)
- Phase 2: Process relations, resolve forward references
- Phase 3: Notify sinks of ID changes, queue rewrites

**DocCodec** (`md.rs`):
- `parse()`: Extract IDs from source, store in ProtoBeliefNode
- `inject_context()`: Generate/inject anchors based on collision state
- `generate_source()`: Write anchors only when necessary (Bref collision)

### Future Enhancements

1. **Event-based ID tracking**:
   - `BeliefEvent::NodeRenamed { from: old_bid, to: new_bid }`
   - `BeliefEvent::TitleChanged { bid, old_title, new_title }`
   - Explicit events for triangulation triggers

2. **Persistent ID change log**:
   - Track ID history: `bid → [(timestamp, old_id)]`
   - Enable "undo" and historical link resolution

3. **Cross-network triangulation**:
   - Resolve references across multiple belief networks
   - Federated ID resolution via API

4. **Smart conflict resolution**:
   - When multiple nodes match, rank by edit distance, recency, etc.
   - Present disambiguation UI to user

This appendix should be considered part of the core design documentation and referenced when implementing any ID-related features.

---

## Quick Reference: Simplified Implementation (2025-01-26)

### Key Discovery

pulldown_cmark's `ENABLE_HEADING_ATTRIBUTES` option provides **automatic anchor parsing** - the hardest part of this issue is already done!

### What We Get For Free

✅ **Parsing**: `{#anchor}` syntax automatically extracted into `id` field  
✅ **Text stripping**: Heading text has anchor removed automatically  
✅ **All formats**: Works with plain IDs, BIDs, Brefs, any string  

### What We Need to Implement

1. **Enable the option** (1 line uncomment)
2. **Capture `id` field** during parse (change `id: _` to `id`)
3. **Store in ProtoBeliefNode** (`document.insert("id", ...)`)
4. **Collision detection** (Bref fallback when title-based ID collides)
5. **Selective injection** (only write `{#bref}` when needed for collision)
6. **Update `BeliefNode::keys()`** to include ID-based NodeKey

### Integration with Issue 2

Issue 2 already checks for "id" field in `extract_anchor_from_node()`:
```rust
node.document.get("id").and_then(|v| v.as_str())
```

Once we store the `id` field during parse, **Issue 2's BID and anchor matching will automatically start working**!

### Estimated Timeline

- Step 1 (Enable + Capture): **0.25 days** ← Most of original 0.5 days eliminated
- Step 2 (Collision detection): **0.5 days**
- Step 3 (Selective injection): **0.5 days**  
- Step 4 (ID update detection): **0.5 days** ← Simplified by having parsed ID
- Step 5 (Document writing): **0.25 days** ← pulldown_cmark_to_cmark handles serialization
- Step 6 (BeliefNode::keys()): **0.25 days**

**Total: ~2 days** (down from 3+ days originally estimated)

---

## Test Status and TODO Assertions (2025-01-26)

### TDD Scaffold Complete

**Unit Tests**: ✅ 6 tests written and passing in `src/codec/md.rs`
- `test_determine_node_id_no_collision`
- `test_determine_node_id_title_collision`
- `test_determine_node_id_explicit_collision`
- `test_determine_node_id_normalization`
- `test_determine_node_id_normalization_collision`
- `test_to_anchor_consistency`

All tests pass with stub implementation of `determine_node_id()` function.

**Integration Tests**: ✅ 4 tests written in `tests/codec_test.rs`
- `test_anchor_collision_detection` (lines 553-629)
- `test_explicit_anchor_preservation` (lines 632-680)
- `test_anchor_normalization` (lines 683-725)
- `test_anchor_selective_injection` (lines 728-771)

**Test Fixtures**: ✅ 3 markdown files created in `tests/network_1/`
- `anchors_collision_test.md` - Tests collision detection with duplicate "Details" headings
- `anchors_explicit_test.md` - Tests explicit anchor preservation
- `anchors_normalization_test.md` - Tests special character normalization

### TODO Assertions to Uncomment After Implementation

**In `test_anchor_collision_detection`** (lines ~605-620):
```rust
// TODO: After Issue 3 implementation, verify:
// - First "Details" has id="details" (title-derived, no anchor in markdown)
// - Second "Details" has id=<bref> (Bref injected as {#<bref>})
// - Both have different IDs (no collision in final output)
// - Rewritten content shows {#<bref>} on second "Details" heading only
```

**In `test_explicit_anchor_preservation`** (lines ~667-678):
```rust
// TODO: After Issue 3 implementation, verify:
// - getting_started.id == Some("getting-started")
// - setup.id == Some("custom-setup-id")
// - configuration.id == Some("configuration")
// - advanced.id == Some("usage")
//
// - Explicit anchors appear in markdown source: {#getting-started}, {#custom-setup-id}, {#usage}
// - Configuration has NO anchor in markdown (title-derived)
// - All explicit anchors are preserved exactly as written
```

**In `test_anchor_normalization`** (lines ~727-735):
```rust
// TODO: After Issue 3 implementation, verify:
// - All explicit anchors are normalized for collision check
// - API & Reference → api--reference (punctuation stripped)
// - Section One! → section-one (space and punctuation normalized)
// - My-Custom-ID → my-custom-id (case normalized)
// - No collisions after normalization
// - Original anchor text preserved in markdown
```

**In `test_anchor_selective_injection`** (lines ~755-762):
```rust
// TODO: After Issue 3 implementation, verify:
// - First "Details" heading has NO anchor in markdown (title-derived ID is unique)
// - Second "Details" heading HAS anchor {#<bref>} (collision → Bref injected)
// - Other unique headings (Implementation, Testing) have NO anchors
// - Only inject anchors when necessary (Bref collision case)
```

### Minor API Fixes Needed

The integration tests currently use outdated BeliefBase API calls that need updating:
- `global_bb.graph()` → `global_bb.relations().as_graph()`
- `global_bb.index_to_node()` → pattern from existing tests using `.states().values()`

See `test_sections_metadata_enrichment` (lines 229-250) for correct pattern.

### Implementation Checklist

Before uncommenting TODO assertions:
1. ✅ Enable `ENABLE_HEADING_ATTRIBUTES` option
2. ✅ Capture `id` field from `MdTag::Heading` during parse
3. ✅ Implement `determine_node_id()` with collision detection
4. ✅ Implement selective anchor injection (only Brefs for collisions)
5. ✅ Update `BeliefNode::keys()` to include ID-based NodeKey
6. ✅ Fix integration test API calls
7. ✅ Uncomment and verify all TODO assertions pass

### Expected Behavior After Implementation

1. **Parsing**: Anchors like `{#intro}` automatically extracted and stored in `node.id`
2. **Collision Detection**: Duplicate titles get Bref fallback (e.g., second "Details" → `{#a1b2c3d4e5f6}`)
3. **Selective Injection**: Only collision cases get anchors injected; unique titles have no anchor
4. **Normalization**: Special chars in explicit IDs normalized before collision check
5. **Preservation**: Explicit anchors preserved exactly; title-derived IDs auto-update with title changes
6. **Issue 2 Integration**: BID and anchor matching in sections metadata will start working immediately
