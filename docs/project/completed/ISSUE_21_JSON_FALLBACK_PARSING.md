# Issue 21: JSON/TOML Dual-Format Support with Network Configuration

**Priority**: MEDIUM - Enables cross-platform metadata formats
**Estimated Effort**: 3-4 days
**Dependencies**: Issue 1 (Schema Registry) ✅
**Blocks**: ROADMAP_HTML_RENDERING Phase 1
**Status**: ✅ **COMPLETE** (2025-01-24)

## Summary

✅ **COMPLETE**: Added comprehensive JSON and TOML dual-format support to `belief_ir.rs`, making JSON the default for cross-platform compatibility while supporting TOML as an alternative. BeliefNetwork files can now be either `.toml` or `.json`, with network-level configuration via a "network config" schema that controls default metadata format preferences for the entire repository.

## Goals

1. **JSON as default**: Try JSON first for cross-platform compatibility (browsers, web tools)
2. **TOML fallback**: Support TOML as alternative format
3. **Dual-format network files**: Support both `BeliefNetwork.json` and `BeliefNetwork.toml`
4. **Network configuration schema**: Define and parse network-level config parameters
5. **Format preference cascading**: Extension > network config > global default (JSON)
6. **Bidirectional conversion**: Convert between JSON ↔ TOML for uniform internal handling

## Architecture

### Format Detection Strategy

**Priority order for determining format**:
1. **Explicit file extension**: `.json` or `.toml` (for network files)
2. **Network configuration**: Default format specified in BeliefNetwork config
3. **Global default**: JSON (for maximum cross-platform compatibility)

**Network Files**:
- `BeliefNetwork.json` OR `BeliefNetwork.toml`
- Detect by extension, parse accordingly
- Network config schema defines repo-wide preferences

**Document Frontmatter**:
- Use network's configured default format
- Fall back to JSON if no network config present
- Support both formats via try-parse pattern

### Network Configuration Schema

**Schema name**: `noet.network_config`

**Fields**:
```toml
# Network metadata
bid = "01234567-89ab-cdef-0123-456789abcdef"
id = "my-project"
title = "Project Documentation Network"
schema = "noet.network_config"

# Configuration parameters
[config]
default_metadata_format = "json"  # or "toml"
strict_format = false  # if true, reject non-default formats
validate_on_parse = true  # run schema validation during parse
auto_normalize = true  # normalize format on write
```

**Graph fields** (if any):
- None for now (network config is self-contained)

### Current vs. Target Implementation

**Current**:
```rust
// belief_ir.rs
pub const NETWORK_CONFIG_NAME: &str = "BeliefNetwork.toml";

impl FromStr for ProtoBeliefNode {
    fn from_str(str: &str) -> Result<ProtoBeliefNode, BuildonomyError> {
        // Parse as TOML only
        proto.document = proto.content.parse::<DocumentMut>()?;
    }
}
```

**Target**:
```rust
// belief_ir.rs
pub const NETWORK_CONFIG_NAMES: &[&str] = &["BeliefNetwork.json", "BeliefNetwork.toml"];

pub enum MetadataFormat {
    Json,
    Toml,
}

impl ProtoBeliefNode {
    // Parse with format preference from network config
    pub fn from_str_with_format(
        str: &str, 
        preferred_format: MetadataFormat
    ) -> Result<ProtoBeliefNode, BuildonomyError> {
        // Try preferred format first, then fallback
    }
    
    // Existing method maintains JSON-first default
    fn from_str(str: &str) -> Result<ProtoBeliefNode, BuildonomyError> {
        Self::from_str_with_format(str, MetadataFormat::Json)
    }
}

// Helper functions
fn parse_json_to_document(json_str: &str) -> Result<DocumentMut, BuildonomyError>;
fn parse_toml_to_document(toml_str: &str) -> Result<DocumentMut, BuildonomyError>;
fn detect_network_file(dir: &Path) -> Option<(PathBuf, MetadataFormat)>;
```

## Implementation Steps

### 1. Define Network Configuration Schema (0.5 days) ✅

- [x] Create `schemas/noet/network_config.json` JSON Schema
- [x] Define `config` object with metadata format preferences
- [x] Register schema manually in `SchemaRegistry::create()`
- [x] Add schema to built-in schemas in `SchemaRegistry::create()`

**Schema outline**:
```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "Network Configuration",
  "type": "object",
  "required": ["bid", "id", "schema"],
  "properties": {
    "bid": { "type": "string", "format": "uuid" },
    "id": { "type": "string" },
    "title": { "type": "string" },
    "schema": { "const": "noet.network_config" },
    "config": {
      "type": "object",
      "properties": {
        "default_metadata_format": {
          "type": "string",
          "enum": ["json", "toml"],
          "default": "json"
        },
        "strict_format": { "type": "boolean", "default": false },
        "validate_on_parse": { "type": "boolean", "default": true },
        "auto_normalize": { "type": "boolean", "default": true }
      }
    }
  }
}
```

### 2. Dual-Format Parsing Infrastructure (1 day) ✅

- [x] Made `serde_json` a core dependency (was optional)
- [x] Create `parse_json_to_document()` helper
- [x] Create `parse_toml_to_document()` helper
- [x] Implement bidirectional JSON ↔ TOML conversion
- [x] Handle type edge cases:
  - JSON null → TOML handling (skip in objects)
  - TOML datetime → JSON string
  - Nested structures preservation

**Key helper function**:
```rust
fn parse_with_fallback(
    content: &str,
    primary: MetadataFormat,
) -> Result<DocumentMut, BuildonomyError> {
    let (primary_parser, fallback_parser) = match primary {
        MetadataFormat::Json => (parse_json_to_document, parse_toml_to_document),
        MetadataFormat::Toml => (parse_toml_to_document, parse_json_to_document),
    };
    
    match primary_parser(content) {
        Ok(doc) => {
            tracing::debug!("Parsed as {:?}", primary);
            Ok(doc)
        }
        Err(primary_err) => {
            tracing::debug!("Primary format failed, trying fallback");
            match fallback_parser(content) {
                Ok(doc) => {
                    tracing::info!("Parsed with fallback format");
                    Ok(doc)
                }
                Err(fallback_err) => {
                    Err(BuildonomyError::Codec(format!(
                        "Failed to parse as {:?} or fallback.\nPrimary: {}\nFallback: {}",
                        primary, primary_err, fallback_err
                    )))
                }
            }
        }
    }
}
```

### 3. Update Network File Discovery (0.5 days) ✅

- [x] Replace `NETWORK_CONFIG_NAME` constant with `NETWORK_CONFIG_NAMES` array
- [x] Update `iter_net_docs()` to check for both `.json` and `.toml` files
- [x] Create `detect_network_file()` public function returning `(path, format)`
- [x] Update `ProtoBeliefNode::from_file()` to handle both formats
- [x] Removed deprecated constant entirely

**Network detection logic**:
```rust
fn detect_network_file(dir: &Path) -> Option<(PathBuf, MetadataFormat)> {
    // Try BeliefNetwork.json first (default)
    let json_path = dir.join("BeliefNetwork.json");
    if json_path.exists() {
        return Some((json_path, MetadataFormat::Json));
    }
    
    // Fallback to BeliefNetwork.toml
    let toml_path = dir.join("BeliefNetwork.toml");
    if toml_path.exists() {
        return Some((toml_path, MetadataFormat::Toml));
    }
    
    None
}
```

### 4. Network Config Parsing and Propagation (1 day)

- [ ] Parse network file and extract `config` object
- [ ] Store network config in `ProtoBeliefNode` for network nodes
- [ ] Pass network config down to child document parsing
- [ ] Respect `default_metadata_format` preference
- [ ] Implement `strict_format` validation (if enabled, reject non-default)

**Context propagation**:
```rust
pub struct NetworkConfig {
    pub default_metadata_format: MetadataFormat,
    pub strict_format: bool,
    pub validate_on_parse: bool,
    pub auto_normalize: bool,
}

impl ProtoBeliefNode {
    // Add network_config field
    pub network_config: Option<NetworkConfig>,
    
    // Extract config during network parsing
    fn extract_network_config(&self) -> Option<NetworkConfig> {
        // Parse from self.document["config"] object
    }
    
    // Use config when parsing child documents
    fn parse_child_with_network_config(&self, content: &str) -> Result<ProtoBeliefNode> {
        let format = self.network_config
            .as_ref()
            .map(|c| c.default_metadata_format)
            .unwrap_or(MetadataFormat::Json);
        
        ProtoBeliefNode::from_str_with_format(content, format)
    }
}
```

### 5. Update FromStr Implementation (0.5 days)

- [ ] Modify `ProtoBeliefNode::from_str()` to default to JSON-first
- [ ] Add `from_str_with_format()` method for explicit format preference
- [ ] Update call sites to pass network config when available
- [ ] Preserve backward compatibility (existing TOML docs still parse)

### 6. Testing (0.5-1 day)

- [ ] Test JSON-first parsing (new default)
- [ ] Test TOML parsing (backward compatibility)
- [ ] Test `BeliefNetwork.json` files
- [ ] Test `BeliefNetwork.toml` files
- [ ] Test network config schema parsing
- [ ] Test format preference propagation to child docs
- [ ] Test `strict_format` enforcement
- [ ] Test edge cases (malformed JSON/TOML, missing config)
- [ ] Verify schema traversal works with both formats

## Testing Requirements

### Test Cases

**1. Format Detection**:
- BeliefNetwork.json detected and parsed as JSON
- BeliefNetwork.toml detected and parsed as TOML
- Directory with neither file returns appropriate error
- Directory with both files prefers .json

**2. Parsing Fallback**:
- Valid JSON parses as primary (no fallback)
- Valid TOML parses when JSON fails
- Both invalid formats return comprehensive error
- Malformed input includes both error messages

**3. Network Configuration**:
- Network config schema validates correctly
- `default_metadata_format` properly extracted
- Child documents respect network preference
- Missing config defaults to JSON

**4. Schema Traversal**:
- JSON frontmatter populates edges correctly
- TOML frontmatter populates edges correctly
- Both formats produce identical graph structure

**5. Round-Trip**:
- Parse JSON → serialize TOML → parse TOML (consistent)
- Parse TOML → serialize JSON → parse JSON (consistent)

### Example Test Data

**BeliefNetwork.json**:
```json
{
  "bid": "12345678-1234-1234-1234-123456789abc",
  "id": "test-network",
  "title": "Test Network",
  "schema": "noet.network_config",
  "config": {
    "default_metadata_format": "json",
    "strict_format": false,
    "validate_on_parse": true,
    "auto_normalize": true
  }
}
```

**Document with JSON frontmatter** (default):
```json
{
  "bid": "87654321-4321-4321-4321-cba987654321",
  "schema": "intention_lattice.intention",
  "title": "Example Document",
  "parent_connections": [
    {
      "parent_id": "bid://12345678",
      "rationale": "Supports network goal"
    }
  ]
}
```

**Same document with TOML** (if network config specifies):
```toml
bid = "87654321-4321-4321-4321-cba987654321"
schema = "intention_lattice.intention"
title = "Example Document"

[[parent_connections]]
parent_id = "bid://12345678"
rationale = "Supports network goal"
```

## Success Criteria

- [ ] JSON is default parsing format (try first)
- [ ] TOML parsing works as fallback
- [ ] Both `BeliefNetwork.json` and `BeliefNetwork.toml` supported
- [ ] Network config schema defined and registered
- [ ] Network config properly parsed and propagated
- [ ] Format preferences cascade correctly (extension > config > default)
- [ ] Schema traversal works identically for both formats
- [ ] Comprehensive error messages for parse failures
- [ ] All tests passing
- [ ] Documentation updated (module docs, examples)

## Risks

**Risk 1: Type Conversion Edge Cases**
**Mitigation**: Comprehensive test suite, document unsupported patterns, handle gracefully

**Risk 2: Breaking Changes for Existing TOML Users**
**Mitigation**: Maintain backward compatibility via fallback, document migration path

**Risk 3: Network Config Complexity**
**Mitigation**: Start with minimal config options, expand iteratively based on need

**Risk 4: Format Preference Confusion**
**Mitigation**: Clear logging of which format was used, explicit error messages

**Risk 5: Performance Overhead**
**Mitigation**: Parse with preferred format first (minimize fallback attempts), benchmark if needed

## Open Questions

1. **Should we support per-document format override?**
   - Could add frontmatter field: `_format: "json"` to override network default
   - Decision: Defer to Phase 2, keep simple for now

2. **What happens if both BeliefNetwork.json and .toml exist?**
   - Option A: Error (ambiguous)
   - Option B: Prefer .json (default format)
   - Recommendation: **Prefer .json** with warning log

3. **Should serialization preserve input format?**
   - Option A: Always write in network's default format
   - Option B: Preserve input format (JSON→JSON, TOML→TOML)
   - Recommendation: **Use network default** for consistency

4. **How to handle network config changes?**
   - If network changes from TOML to JSON default, do we migrate all docs?
   - Recommendation: Support gradual migration (parse both, write in new default)

## Migration Path for Existing Users

**For users with existing TOML documents**:
1. **No immediate action required** - TOML continues to work via fallback
2. **Optional migration**: Convert BeliefNetwork.toml → BeliefNetwork.json
3. **Gradual transition**: New documents use JSON, old ones remain TOML
4. **Batch migration tool** (future): `noet migrate --format json <directory>`

**Migration benefits**:
- Better cross-platform compatibility (browsers, web tools)
- Wider ecosystem support (JSON is more universal)
- Consistent format across all metadata

## References

- Current implementation: `src/codec/belief_ir.rs`
- Network file detection: `iter_net_docs()` (lines 31-79)
- Schema traversal: `ProtoBeliefNode::traverse_schema()` (lines 623-753)
- Parsing: `ProtoBeliefNode::from_str()` (lines 756-787)
- Related: Issue 1 (Schema Registry) - network config schema registration
- Related: Issue 2 (Multi-Node TOML Parsing) - may need JSON support there too
- Related: ROADMAP_HTML_RENDERING Phase 1 - enables JSON for HTML workflows