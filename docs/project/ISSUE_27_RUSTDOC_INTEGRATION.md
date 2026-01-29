# Issue 21: Unified Rustdoc and Design Documentation

**Priority**: LOW - Post-v0.2.0 enhancement
**Estimated Effort**: 5-7 days
**Dependencies**: Issue 06 (HTML Generation) complete

## Summary

Cross-link `cargo doc` API documentation with noet-exported design documentation, creating a unified documentation site where users can navigate bidirectionally between "what the code does" (rustdoc) and "why it works this way" (design docs). Enables querying across code and architecture, treating Rust source files as another document format in the BeliefBase.

**Use Cases**:
- Click from `MdCodec` rustdoc page → `architecture.md` design explanation
- Click from design doc "DocCodec trait" → rustdoc API reference
- Query: "Which types implement the concepts described in this design doc?"
- Unified search across API and design documentation

## Goals

1. Enable bidirectional linking between rustdoc and design docs
2. Support BID references in Rust doc comments
3. Parse Rust source files to extract API structure into BeliefBase
4. Generate unified documentation site (rustdoc + design docs HTML)
5. Maintain compatibility with standard `cargo doc` workflow
6. Deploy integrated docs to GitHub Pages automatically

## Architecture

### Overview: Three-Phase Integration

**Phase 1: Manual Linking (Quick Win)**
- Add explicit links in doc comments to design docs
- Add rustdoc references in design doc markdown
- No special tooling required

**Phase 2: BID-Based Cross-Linking**
- Support BID references in doc comments: `{#bid:550e8400-...}`
- Post-process rustdoc HTML to convert BIDs to links
- Update design doc export with rustdoc URLs

**Phase 3: RsCodec (Full Integration)**
- Parse `.rs` files as documents with RsCodec
- Extract modules, structs, functions, traits into BeliefBase
- Enable queries across code and design docs
- Generate unified documentation site

### Phase 1: Manual Linking (Immediate)

**In Rust source:**
```rust
/// Parses markdown documents into belief nodes.
///
/// This implements the DocCodec trait described in the
/// [architecture documentation](https://buildonomy.github.io/noet-core/design/architecture.html).
///
/// For the complete parsing pipeline, see
/// [beliefbase_architecture.md](https://buildonomy.github.io/noet-core/design/beliefbase_architecture.html).
pub struct MdCodec {
    // ...
}
```

**In design docs:**
```markdown
## DocCodec Implementation

See API reference: [`MdCodec`](https://docs.rs/noet-core/latest/noet_core/codec/struct.MdCodec.html)
```

**Benefits**: Works immediately, no tooling changes needed

### Phase 2: BID-Based Cross-Linking

**BID references in doc comments:**
```rust
/// Parses markdown documents into belief nodes.
///
/// Design documentation: {#bid:550e8400-e29b-41d4-a716-446655440000}
///
/// Related concepts:
/// - Compilation pipeline: {#bref:abc123def456}
/// - Multi-pass resolution: {#bid:789abc-...}
pub struct MdCodec {
    // ...
}
```

**Implementation**:
1. Generate rustdoc HTML normally: `cargo doc`
2. Post-process HTML to find BID references
3. Resolve BIDs against BeliefBase (from design doc export)
4. Replace with links: `{#bid:...}` → `<a href="/design/architecture.html#section">Architecture</a>`
5. Update design docs with rustdoc backlinks

**Data Structure**:
```rust
pub struct RustdocLinker {
    belief_base: BeliefBase,
    rustdoc_root: PathBuf,
    design_docs_root: PathBuf,
}

impl RustdocLinker {
    /// Post-process rustdoc HTML to resolve BID references
    pub fn link_rustdoc(&self) -> Result<()> {
        for html_file in self.rustdoc_html_files() {
            let content = fs::read_to_string(&html_file)?;
            let linked = self.resolve_bids_in_html(&content)?;
            fs::write(&html_file, linked)?;
        }
        Ok(())
    }
    
    /// Find BID references in HTML: {#bid:...} or {#bref:...}
    fn resolve_bids_in_html(&self, html: &str) -> Result<String> {
        let bid_regex = Regex::new(r"\{#(bid|bref):([a-f0-9-]+)\}")?;
        
        bid_regex.replace_all(html, |caps: &Captures| {
            let key = match &caps[1] {
                "bid" => NodeKey::Bid { bid: Bid::parse(&caps[2]).unwrap() },
                "bref" => NodeKey::Bref { bref: Bref::from_str(&caps[2]).unwrap() },
                _ => return caps[0].to_string(),
            };
            
            if let Some(node) = self.belief_base.resolve(&key) {
                let url = self.design_docs_url(&node);
                format!(r#"<a href="{}">{}</a>"#, url, node.title)
            } else {
                caps[0].to_string() // Keep original if unresolved
            }
        }).to_string()
    }
}
```

### Phase 3: RsCodec (Full Integration)

**Rust source as document format:**
```rust
// src/codec/rs.rs

use syn::{File, Item, ItemMod, ItemStruct, ItemFn};

pub struct RsCodec {
    parsed_items: Vec<ProtoBeliefNode>,
}

impl DocCodec for RsCodec {
    fn parse(&mut self, content: String, current: ProtoBeliefNode) -> Result<()> {
        // Parse Rust source with syn
        let syntax_tree: File = syn::parse_str(&content)?;
        
        // Extract modules, structs, functions
        for item in syntax_tree.items {
            match item {
                Item::Mod(module) => self.parse_module(module, &current)?,
                Item::Struct(struct_) => self.parse_struct(struct_, &current)?,
                Item::Fn(func) => self.parse_function(func, &current)?,
                Item::Trait(trait_) => self.parse_trait(trait_, &current)?,
                _ => {}
            }
        }
        
        Ok(())
    }
    
    fn nodes(&self) -> Vec<ProtoBeliefNode> {
        self.parsed_items.clone()
    }
}

impl RsCodec {
    fn parse_struct(&mut self, struct_: ItemStruct, parent: &ProtoBeliefNode) -> Result<()> {
        let doc_comment = self.extract_doc_comment(&struct_.attrs);
        
        let node = ProtoBeliefNode {
            kind: NodeKind::Section, // Or custom "RustStruct" kind
            title: struct_.ident.to_string(),
            payload: toml::map! {
                "type" => "struct",
                "visibility" => self.visibility_string(&struct_.vis),
                "doc_comment" => doc_comment,
            },
            ..parent.clone()
        };
        
        self.parsed_items.push(node);
        Ok(())
    }
    
    fn extract_doc_comment(&self, attrs: &[Attribute]) -> String {
        attrs.iter()
            .filter_map(|attr| {
                if attr.path().is_ident("doc") {
                    attr.parse_args::<LitStr>().ok().map(|s| s.value())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
```

**Benefits**:
- Rust source files become queryable in BeliefBase
- Extract API structure automatically
- Cross-references between code and design maintained in graph
- Enables powerful queries: "Show all implementations of this design concept"

### Unified Documentation Site Structure

```
/docs/                              (GitHub Pages root)
  ├── api/                          (rustdoc output)
  │   ├── noet_core/
  │   │   ├── codec/
  │   │   │   └── struct.MdCodec.html  ← Links to design/architecture.html
  │   │   └── beliefbase/
  │   └── index.html
  │
  ├── design/                       (noet HTML export)
  │   ├── architecture.html         ← Links to api/noet_core/codec/struct.MdCodec.html
  │   ├── beliefbase_architecture.html
  │   └── index.html
  │
  └── index.html                    (Landing page with unified search)
```

## Implementation Steps

### Phase 1: Manual Linking (1 day)

- [ ] Add explicit links in doc comments to design docs
- [ ] Add rustdoc references in design markdown
- [ ] Update GitHub Actions to deploy both rustdoc and design docs
- [ ] Create landing page linking to both doc sets

### Phase 2: BID-Based Cross-Linking (2-3 days)

- [ ] Implement `RustdocLinker` to post-process rustdoc HTML
- [ ] Add BID reference regex matching: `{#bid:...}` and `{#bref:...}`
- [ ] Resolve BIDs against BeliefBase from design doc export
- [ ] Replace BID references with HTML links
- [ ] Generate backlinks in design docs to rustdoc
- [ ] Update CI to run post-processing after `cargo doc`

### Phase 3: RsCodec Implementation (3-4 days)

- [ ] Create `RsCodec` that parses Rust source with `syn`
- [ ] Extract modules, structs, functions, traits into ProtoBeliefNodes
- [ ] Parse doc comments and extract BID references
- [ ] Register RsCodec for `.rs` file extension
- [ ] Generate unified BeliefBase including code + design
- [ ] Export unified HTML with bidirectional links
- [ ] Enable queries across code and documentation

### Phase 4: Unified Search and Navigation (2 days)

- [ ] Create landing page with search across both doc sets
- [ ] Implement client-side search (WASM BeliefBase from Issue 06)
- [ ] Add navigation sidebar linking API and design sections
- [ ] Style consistently across rustdoc and design docs

## Testing Requirements

### Manual Testing

- [ ] Verify links from rustdoc to design docs work
- [ ] Verify links from design docs to rustdoc work
- [ ] Test BID resolution in doc comments
- [ ] Validate unified search finds results in both doc sets
- [ ] Check navigation between API and design sections

### Integration Tests

- [ ] RsCodec parsing of example Rust files
- [ ] BID reference resolution in rustdoc HTML
- [ ] Unified BeliefBase construction (code + design)
- [ ] Link generation for all node types

### CI/CD Testing

- [ ] GitHub Actions successfully deploys unified docs
- [ ] Post-processing completes without errors
- [ ] All cross-links are valid (no 404s)

## Success Criteria

- [ ] Doc comments can reference design docs via BIDs
- [ ] Design docs can reference API types via rustdoc URLs
- [ ] Bidirectional linking works in both directions
- [ ] Unified documentation site deployed to GitHub Pages
- [ ] Search works across API and design documentation
- [ ] RsCodec can parse Rust source into BeliefBase (Phase 3)
- [ ] Queries can traverse code and design nodes (Phase 3)
- [ ] Documentation stays synchronized with code changes

## Risks

**Risk 1: Rustdoc HTML Structure Changes**
- **Impact**: Post-processing breaks when rustdoc output format changes
- **Mitigation**: Use stable rustdoc JSON output when available, version-check rustdoc

**Risk 2: BID Reference Syntax Conflicts**
- **Impact**: `{#bid:...}` syntax might conflict with rustdoc or markdown
- **Mitigation**: Use unique syntax, escape properly, document clearly

**Risk 3: RsCodec Maintenance Burden**
- **Impact**: Must keep up with Rust syntax changes (new features, editions)
- **Mitigation**: Use `syn` crate (maintained by Rust community), defer Phase 3 if needed

**Risk 4: Performance with Large Codebases**
- **Impact**: Parsing all `.rs` files into BeliefBase could be slow
- **Mitigation**: Make optional, cache results, parallelize parsing

## Open Questions

1. **BID reference syntax**: Use `{#bid:...}` or different format?
   - **Recommendation**: `{#bid:...}` for consistency with markdown links

2. **Rustdoc JSON vs HTML**: Post-process HTML or use JSON output?
   - **Recommendation**: Start with HTML (stable), migrate to JSON when ready

3. **RsCodec scope**: Parse all source or just `lib.rs` + public APIs?
   - **Recommendation**: Start with public items only, expand later

4. **Documentation sync**: How to keep code docs synchronized with design docs?
   - **Recommendation**: CI validation (fail if BID references unresolved)

5. **Private items**: Should RsCodec include private modules/functions?
   - **Recommendation**: Make configurable, default to public only

## Future Work (Post-Issue 21)

- **IDE integration**: RsCodec in LSP for live doc linking in editors
- **Design-first workflow**: Generate Rust stub from design docs
- **Traceability matrix**: Visualize code-to-design coverage
- **Doc tests in design docs**: Execute code examples from design docs
- **Version comparison**: Show API changes alongside design doc diffs (Issue 20 integration)

## References

- Issue 06: HTML Generation and Interactive Viewer
- Issue 11-12: LSP Implementation (potential integration point)
- Issue 20: Git-Aware Networks (version comparison synergy)
- Rustdoc JSON: https://doc.rust-lang.org/rustdoc/json.html
- `syn` crate: https://docs.rs/syn/ (Rust parser)
- `cargo doc`: https://doc.rust-lang.org/cargo/commands/cargo-doc.html