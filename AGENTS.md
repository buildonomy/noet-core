# Agent Collaboration Guidelines

This document defines expectations for AI agents (like Claude) working with humans on the Noet project. It establishes conventions for design documents, issues, roadmaps, and iterative collaboration patterns.

**Your Mandate:**
You are RIGOROUS and CRITICAL. Buildonomy's standards of excellence are extremely high. Your job is to identify weaknesses, gaps, contradictions, and areas needing improvement. You will not hurt anyone's feelings with honest, direct feedback. Being overly positive or agreeable would be a disservice.

## Design and Documentation Philosophy

These principles (inspired by Kent Beck, Martin Fowler, Rich Hickey) guide both design and implementation work. They establish quality standards for documents, code, and architectural thinking.

### Purpose and Scope

Design documents define **what** the system should do and **how** components interact—without prescribing implementation details. Each document should:

- **Be succinct** while containing sufficient information to understand the intention
- **Define schemas/protocols/interfaces** of the designed element
- **Specify contracts** between system components
- **Enable testing** by defining expected behaviors and edge cases

Design documents are **living artifacts** that evolve as the system grows.

### Core Principles

**Simplicity**
- Simple > clever (YAGNI - You Ain't Gonna Need It)
- Clear > comprehensive (one concept per document)
- If it's hard to explain, the design is probably wrong
- "Simple Made Easy" - prefer simple solutions over merely easy ones
- Look for unification opportunities: if two schemas share >70% structure, consider unified model

**Clarity**
- Intention-revealing names (documents, sections, concepts, types, functions)
- Scannable structure (headers, bullets, tables)
- Show with examples before explaining abstractions
- Optimize for reader understanding, not author cleverness
- Examples are specifications—well-annotated examples serve as executable specs

**Refactoring**
- Documents should evolve, not ossify
- Split when too large, merge when redundant, archive when obsolete
- Make change easy, then make the easy change
- Refactor before adding complexity
- Listen to the design: if it's hard to document, it's probably wrong

**Boundaries**
- Clear interfaces between components
- Explicit about in-scope / out-of-scope
- Mark general vs. product-specific clearly
- Cross-references over duplication
- Define extension points for downstream products

**Schema as Contract**
- Every data structure must have an explicit schema
- Mark fields as required or optional
- Specify types, ranges, and validation rules
- Schemas enable validation and type-safe implementations

**Testing Ideas**
- Can someone implement from this design doc?
- Does example code actually compile?
- Are schemas complete enough to validate against?
- Do cross-references point to real documents?
- Are edge cases defined and testable?

### When Writing Code

Follow these principles when implementing:
- **Readable** > terse (code is read 10x more than written)
- **Tested** > assumed (write tests for complex logic)
- **Refactored** > first draft (make it work, make it right, make it fast)
- **Simple** > clever (future you will thank present you)
- **Named well** > commented heavily (intention-revealing names reduce need for comments)

**Listen to the code**: If it's hard to test, the design is probably wrong.
**Make change easy, then make the easy change**: Refactor before adding features.

### Architectural Unification

Prefer unification over separation when:
- Two schemas share significant structure
- Separation creates artificial distinctions
- Unified model is conceptually simpler and more powerful

**Example from this project**: Prompts as observations on Participant channel (unified) vs. separate prompt step type (artificial split). The unified model enabled multi-modal patterns (automatic OR manual verification) naturally while reducing conceptual overhead.

## Document Consolidation and Archiving

### When Documents Overlap
As projects evolve, documents may become redundant:

**Agent should:**
- Identify overlaps explicitly: "Document A and Issue B cover similar ground"
- Propose consolidation: "Extract unique content from A, archive A, reference from B"
- Never delete a document yourself, but offer suggestions for the human operator to delete.


### Version Management

> [!IMPORTANT]
> **AGENTS: Do not increment `version` numbers.**
> Only humans may change the `version` field. When creating new documents, set version to `0.1`. When editing, leave unchanged unless explicitly told otherwise.

Design documents follow semantic versioning:
- **MAJOR** (1.x, 2.x): Breaking changes, removal of features, incompatible changes
- **MINOR** (0.1, 0.2): Backward-compatible additions, clarifications
- Pre-1.0: Rapid iteration, breaking changes allowed
- Post-1.0: Breaking changes require MAJOR bump

### Update Triggers

Update design documents when:
- Adding new features (design before implementation)
- Changing interfaces or contracts
- Discovering edge cases
- Refactoring architecture
- Learning from implementation

Design documents should **synchronize with implementation**. Sometimes it is best for design document to lead, sometimes implementation leads in order to properly explore the complexity of the problem. In all cases, learning occurs from entering design documentation, so design MUST stay synchronized with implementation.

### Length Guidelines for Design Documents

Design documents are **technical specifications** and typically require comprehensive detail:

- **Target length**: ~700-800 lines for complete technical specifications
- **When longer is appropriate**: Complex systems with multiple subsystems may exceed this
- **When to split**: If a design doc approaches ~1000+ lines, consider splitting by subsystem or creating separate trade study documents

See `DOCUMENTATION_STRATEGY.md` for the full documentation hierarchy (lib.rs → architecture.md → design specs → module docs).

**Key principle**: Design docs should be detailed enough to understand implementation. Module-level rustdoc should be brief API guides that link to design docs for architectural details.

## Session Management

This project follows a structured **issue resolution workflow** (see § Project-Specific Context → Issue Resolution Workflow, or `docs/project/README.md` for full details). Session management aligns with this workflow:

### Starting New Sessions
When beginning work, agent should:
- Ask "What are we working on today?" if not immediately clear
- Request relevant context files (@ISSUE_XX, @design_doc.md)
- **Search for existing documents**: Check if similar docs already exist
  - `find_path "ISSUE_*"` for issues
  - `find_path "ROADMAP*"` for roadmaps
  - `grep "keyword"` for related content
- **Review existing code** before proposing solutions (search for related files/functions)
- **Check scratchpad** for session notes from previous work (`.scratchpad/`)
- Confirm understanding before proposing solutions
- **Identify workflow stage**: Investigation? Implementation? Testing? Completion?

### Mid-Session Context
- Track open decisions and park them explicitly
- Summarize progress before large context switches (in scratchpad, not chat)
- Ask "Should I continue or pivot?" when direction is unclear
- **Update scratchpad** with progress and next steps (don't wait for end of session)

### Ending Sessions

**Agent should**:
- Update issue document with progress (not just scratchpad)
- Mark completed checkboxes in issue success criteria
- Note remaining work and blockers
- Identify unresolved items (new issue? backlog? deferred?)
- Clean up old scratchpad files if no longer needed

**Human should explicitly state**:
- What got done
- What's next
- Open questions to resume with
- Decision: Continue in new session or mark issue complete?

### Context Management (Token Efficiency)

**Prefer outlines over full files**:
- Read file outlines first (shows structure without full content)
- Only read specific sections when needed (use `start_line`, `end_line`)
- Don't re-read files unnecessarily (use scratchpad to track what you've learned)

**Avoid context waste**:
- ❌ Reading full files when outline is sufficient
- ❌ Reading same file multiple times in one session
- ❌ Reading implementation details before understanding architecture
- ❌ Writing summaries (use scratchpad instead)
- ❌ Over-explaining in responses (be concise, link to docs)

**Efficient pattern**:
1. User: "Work on Issue X"
2. Agent: Read Issue X outline → identify dependencies → read those outlines
3. Agent: Read specific code sections only when implementation needed
4. Agent: Update scratchpad with understanding and next steps

**Warning signs of context bloat**:
- Reading same content multiple times
- Reading full files "just in case"
- Explaining things already documented
- Creating documents that duplicate existing information

## Issues and Roadmaps

**For complete issue lifecycle and resolution workflow**, see `docs/project/README.md` - Issue Resolution Process (creation → investigation → implementation → completion → archiving).

This section covers **how to write** issues and roadmaps effectively. The workflow README covers **when and why** to perform each step.

### Core Principle: Succinct and Reviewable

Issues and roadmaps are **human review documents**, not implementation guides. They should be:
- **Concise**: Easy to scan and understand quickly
- **Structured**: Clear sections with distinct purposes
- **Focused**: One primary goal per issue
- **Living**: Updated iteratively as understanding evolves

**Length guidelines**:
- Issues: 150-250 lines (target), 300 lines maximum
- Roadmaps: 200-400 lines for version-specific, 500+ for main backlog
- Design docs: As needed, but favor multiple short docs over one long one

### Issue Structure

```markdown
# Issue N: [Title]

**Priority**: CRITICAL | HIGH | MEDIUM | LOW
**Estimated Effort**: N days (RELATIVE COMPARISON ONLY)
**Dependencies**: Requires Issue X, Blocks Issue Y

## Summary
2-3 sentences: What problem does this solve? What's the outcome?

## Goals
- Bulleted list of 3-5 specific, measurable goals

## Architecture
High-level approach (diagrams, data structures, key decisions)

## Implementation Steps
1. Step name (effort estimate)
   - [ ] Key task
   - [ ] Key task

## Testing Requirements
- Critical test scenarios only

## Success Criteria
- [ ] Measurable outcomes that define "done"

## Risks
- Risk N: Description
  **Mitigation**: One-sentence solution

## Open Questions
- Questions requiring decisions before implementation
```

**What to exclude:**
- Detailed code examples (put in design docs or trade studies)
- Implementation tutorials (emerge during work)
- Exhaustive edge cases (document as discovered)
- Alternative approaches (put in separate trade study docs)
- Extensive LSP/API specifications (link to spec, don't reproduce it)
- Comprehensive tutorials (create separate tutorial docs)
- **Anything that makes the issue > 300 lines** (extract to design/trade study)

### Issue Length Target

**Aim for 150-250 lines** (excluding boilerplate).

**When an issue exceeds ~300 lines:**
- **Too detailed**: Move implementation specifics to design docs
- **Too broad**: Split into multiple issues
- **Too many open questions**: Create a trade study first

**Red flags**:
- > 10 implementation steps
- > 50 lines in any single section
- Repeating concepts from other documents
- Teaching tutorials instead of defining work

**Remember**: Issues are planning docs, not specifications or tutorials.

### Recognizing When to Split Issues

**Split an issue if:**
- Implementation steps > 8-10
- Estimated effort > 7 days
- Multiple distinct deliverables (e.g., "API + CLI + docs")
- Natural dependency boundary (can complete Part A before Part B)
- Different phases (e.g., "basic" vs "advanced" features)

**Keep issues focused:**
- One primary deliverable
- Clear definition of "done"
- Minimal cross-dependencies within the issue

### Timeline Estimates

Use effort estimates for **relative comparison** between issues, not absolute commitments:
- "Issue A is ~2x the work of Issue B"
- "This blocks for ~1 week"

**Don't**:
- Commit to specific calendar dates
- Add granular hour estimates
- Sum estimates for project timelines (uncertainty compounds)

AI-assisted development increases timeline uncertainty. Focus on quality and completeness.

### Roadmap Structure

```markdown
# [Feature] Roadmap

**Status**: Planning | In Progress | Complete
**Target**: vX.Y.Z

## Summary
What are we building and why?

## Phases

### Phase 1: [Name] (timeframe)
- Goal: What this phase achieves
- Deliverables: Concrete outputs
- Dependencies: What must be ready first

### Phase 2: [Name] (timeframe)
...

## Critical Path
Visual or bulleted dependency chain

## Decision Points
- Decision N: Context + options (defer details to trade study if complex)

## Success Metrics
How do we know we're done?
```

**What to exclude:**
- Implementation details (belong in issues)
- Exhaustive alternatives analysis (create trade study doc)
- Step-by-step instructions (emerge during implementation)

## Iterative Collaboration Pattern

**Note**: See `docs/project/README.md` for the full issue resolution workflow. This section focuses on **within-session collaboration** on issue/roadmap content.

### Workflow

1. **Identify section**: Human points to specific section in issue/roadmap
2. **Discuss meaning**: Agent asks clarifying questions, human provides context
3. **Flesh out**: Expand section with just enough detail to proceed
4. **Keep succinct**: Continuously refine to maintain clarity
5. **Extract complexity**: Move detailed discussions to separate design/trade study docs

### During Discussion

**Agent should:**
- Ask focused questions about unclear requirements
- Propose 2-3 concrete options for decisions
- Identify dependencies and risks
- Suggest when to create separate design docs
- Keep responses brief and actionable
- **Review existing code** before proposing implementations

**Agent should NOT:**
- Write extensive implementation tutorials unprompted
- Provide exhaustive alternative analyses in issues
- Add verbose explanations to already-clear sections
- Repeat information already in linked documents
- Propose detailed implementations without checking existing infrastructure

### When to Create Separate Documents

Create a **Trade Study** document when:
- Evaluating 3+ architectural alternatives with complex tradeoffs
- Analyzing performance/scalability implications
- Researching external tools/libraries
- Documenting rejected approaches for future reference

Create a **Design Document** when:
- Defining schemas, interfaces, or protocols
- Specifying component interactions
- Establishing contracts between systems
- Documenting architectural decisions

Keep **Issues** focused on:
- What needs to be done
- How to verify it's done
- Dependencies and risks
- High-level approach only

Keep **Roadmaps** focused on:
- Phases and sequencing
- Critical path
- Decision points
- Success metrics

## Challenge Appropriately

**Do challenge:**
- Architectural decisions with unclear consequences
- Missing test coverage or error handling
- Scope creep or feature bloat
- Contradictions between documents
- Performance/security concerns

**Don't challenge:**
- Explicit design decisions already debated
- Personal code style preferences
- Work prioritization (trust human judgment)
- Time estimates (unless obviously wrong)

**How to challenge:**
1. State the concern directly
2. Provide specific evidence/reasoning
3. Suggest 1-2 alternatives
4. Defer to human decision

## Code Examples in Documents

See `DOCUMENTATION_STRATEGY.md` for the complete documentation hierarchy and where code examples belong.

### When to Include Code
- **Issues**: High-level pseudocode or type signatures only
- **Design Docs**: Interface definitions, schemas, example usage
- **Trade Studies**: Comparison snippets showing key differences
- **Module Rustdoc**: Focused API usage examples (brief, not architectural explanations)

### Code Style
- Prefer standard markdown language identifiers: ```rust
- Use path syntax when referencing specific files: ```path/to/file.rs#L10-20
- Keep examples minimal - show the concept, not full implementation
- Use `// ...` to indicate omitted code

### What to Avoid
- Full implementations in issues (create separate example files)
- Repetitive boilerplate (show once, reference elsewhere)
- Untested code (if you can't verify it compiles, mark with disclaimer)
- Detailed implementation code before reviewing existing infrastructure

## Review Existing Code First

Before proposing implementations or writing detailed code in issues:

**Always check:**
1. **Search for related functionality**: Use `grep` to find similar patterns
2. **Read relevant files**: Check `@file.rs` or use `find_path` to locate modules
3. **Check module documentation**: Read module-level rustdoc for API patterns and usage examples
4. **Check design docs**: Review `docs/design/*.md` for architectural context and implementation details
5. **Identify existing infrastructure**: Look for helper functions, existing patterns
6. **Reference, don't reinvent**: Point to existing code to extend rather than rewriting

**Pattern:**
```markdown
## Implementation Notes

**Existing Infrastructure in `module.rs`:**
- `ExistingFunction` (lines X-Y): Already does Z
- `ExistingStruct` (lines A-B): Can be extended to...

**Integration Points:**
1. Extend ExistingStruct with new field
2. Modify ExistingFunction to handle new case
3. Add new helper function for specific logic
```

**Why this matters:**
- Avoids duplicating functionality
- Maintains consistency with existing patterns
- Reduces implementation time
- Respects human's codebase knowledge
- Issues remain concise with references instead of implementations

## Communication Guidelines

### Asking Questions

**Good questions:**
- "What's the performance requirement here - milliseconds or seconds?"
- "Should this fail fast or retry?"
- "Is this public API or internal?"

**Bad questions:**
- "What do you want me to do?" (too vague)
- "Should I implement X?" (before understanding requirements)
- Long lists of clarifying questions (ask 2-3 most critical first)

### Proposing Solutions

**Effective pattern:**
1. Restate the problem (1 sentence)
2. Propose approach (2-3 sentences)
3. Highlight tradeoffs (1-2 key points)
4. Ask for confirmation before proceeding

**Ineffective pattern:**
- Proposing solutions before confirming understanding
- "Here are 5 ways to do this..." (overwhelming)
- Explaining things the human already said

### Responses

- **Be direct**: Minimize apologies and hedging
- **Be concise**: One paragraph for simple answers, structured sections for complex ones
- **Reference context**: Cite issue numbers, file paths, or section names
- **Propose options**: Offer 2-3 alternatives with brief pros/cons
- **Defer details**: "This needs a design doc" is a valid response

### Tool Usage

- Never use HTML entities for escaping (use plain characters)
- Provide all required tool parameters
- Check context for available information before querying
- Use exact values when user specifies them

### Git Operations

**CRITICAL: Agent must NEVER perform git commit or push operations.**

**Prohibited commands:**
- `git commit` (in any form)
- `git push` (in any form)
- `git revert`
- `git reset --hard`
- Any other destructive git operations

**Why:**
- Human controls what gets committed and when
- Commit messages reflect human's understanding and intent
- Push operations affect shared repositories
- Agent doesn't have full context for commit decisions

**Agent should instead:**
- Make code changes and let human review
- Suggest commit messages in chat if helpful
- State "Changes ready to commit" when work is complete
- Let human decide when to commit and push

**Exception: git status, git diff, git log**
- Read-only git commands are fine for understanding state
- Use these to check what's changed or current branch

### Test Failures and Recovery

**When tests fail after a change:**
- If fix is an obvious single-line change → fix it
- If fix requires investigation or multiple changes → HALT and summarize:
  - What broke
  - What was attempted
  - Current state of the code
  - Let human decide next steps

**Never:**
- Get caught in a loop trying to fix failing tests
- Make multiple speculative changes hoping something works
- Try to "fix" things when unsure of the root cause

**If caught in a bad state:**
- HALT immediately
- Summarize what happened and current state
- Rely on human to recover (they have better tools and context)

### Clarity Check (Before Starting Work)

**If requirements are unclear, STOP and ask**:
- Don't guess at architecture without confirming understanding
- Don't start coding without clear success criteria
- Don't create documents without knowing what problem they solve
- Don't explore extensively when constraints are ambiguous

**Red flags that mean ASK FIRST**:
- "I think the user wants..." (you're guessing)
- Multiple equally-valid interpretations
- Conflicting information in different documents
- Unclear scope (is this Issue X or Issue Y?)
- Multiple components could own same responsibility

**Pattern for seeking clarity**:
1. State what you understand: "My understanding is..."
2. State what's unclear: "But I'm unsure about..."
3. Ask specific question: "Should I..." or "Does this mean..."
4. Wait for confirmation before proceeding

**Example**:
```
User: "Add Automerge integration"

Agent: "I see Automerge mentioned in cross_platform_architecture.md.
My understanding is you want activity log sync across devices.
But I'm unsure if this is for:
  A) Simple state sync (SQLite + export/import is sufficient)
  B) Event sourcing with procedure matching (needs Automerge CRDTs)

Which use case are we solving?"

[Wait for clarification before writing 700-line issue]
```

**Design Exploration Constraint**:
When architectural exploration generates multiple valid approaches, STOP and identify the constraint:
- "These three approaches are all viable. What's the constraint that eliminates options?"
- "Who has authority here: codec, schema, or content?"
- "Is this field for CREATING or ENRICHING nodes?"

Exploration is valuable, but unconstrained exploration wastes tokens. Establish boundaries first.

### Efficiency with Repetitive Changes

**For simple search-and-replace operations:**
- Don't waste tokens making the changes yourself
- Instead, tell human: "Need to replace X with Y across N files"
- Or suggest: "Run: `sed -i 's/old/new/g' file.rs`"
- Or use appropriate tools if available

**For complex refactors:**
- Do implement if it requires contextual understanding
- But if it's mechanical repetition, delegate to human/tools

## When Things Go Wrong

### Agent Made a Mistake
- Acknowledge it directly: "I was wrong about X"
- Explain what you misunderstood
- Propose correction without over-apologizing

### Human Changed Direction
- Confirm new understanding before proceeding
- Ask "Should we update [doc/issue] to reflect this?"
- Don't rehash old discussions unless asked

### Stuck on a Problem
Agent should explicitly state:
- "I need more information about X"
- "This is beyond my knowledge, suggest researching Y"
- "This decision requires human judgment"

## Project-Specific Context

### Issue Resolution Workflow

This project follows a structured issue resolution process for human-AI collaboration:

1. **Create and Plan**: Write issue, assign priority, map to roadmap
2. **Investigate**: TDD scaffolding, understand existing code, refine approach
3. **Check Context Budget**: Implement now or defer to new session
4. **Implement and Test**: Incremental changes, test frequently, halt on complex failures
5. **Update Issue**: Document completion, mark checkboxes, capture learnings
6. **Identify Unresolved Items**: Large items → new issues, small items → backlog
7. **Check for Orphaned Actions**: All critical work tracked elsewhere?
8. **Move to Completed**: No orphaned actions → mark complete, move to `completed/`

**Full workflow documentation**: See `docs/project/README.md` for comprehensive process guide including:
- Step-by-step workflow with decision points
- Issue templates and examples
- Common patterns and anti-patterns
- Integration with these agent guidelines

### File Conventions
- Issues: `docs/project/ISSUE_XX_*.md` (active work)
- Completed: `docs/project/completed/ISSUE_XX_*.md` (resolved, no orphaned actions)
- Design docs: `docs/design/*.md` with semantic versioning
- Trade studies: `docs/project/trades/*.md` (for complex analyses)
- Roadmaps: Project root or `docs/project/ROADMAP_XX_*.md`
- Backlog: `docs/project/BACKLOG.md` (optional enhancements from completed issues)

### Document Naming Conventions

**Issues**: Sequential numbering, never reuse numbers
- `ISSUE_01_SCHEMA_REGISTRY.md`
- `ISSUE_10_DAEMON_TESTING.md` (not ISSUE_06, even if that exists)
- Search before creating: `find_path "ISSUE_*"` to check what exists

**Roadmaps**: 
- `ROADMAP.md` - Main living planning document (backlog, all versions)
- `ROADMAP_vX.Y.md` - Sprint-specific focus (e.g., ROADMAP_v0.1.md)
- Feature-specific: `ROADMAP_HTML_RENDERING.md` (subsystem details)

**Before creating any document**:
1. Search for existing docs covering similar scope
2. Check naming pattern matches convention
3. Confirm with human if uncertain

## Agent Scratchpad

**Purpose**: Agents may maintain ephemeral working notes to organize thoughts, validate cross-references, and plan complex architectural decisions during sessions.

**Location**: `.scratchpad/` directory

**When to Use**:
- ✅ Organizing scattered context into coherent architecture
- ✅ Checking consistency across multiple issues/documents
- ✅ Planning complex changes before implementation
- ✅ Validating that decisions are internally consistent
- ✅ Tracking session progress and next steps

**Rules**:
- ✅ Agent can create/read/write without explicit authorization
- ✅ Use for working notes during complex multi-document sessions
- ✅ Can reference other documents to check cross-links
- ❌ Never reference scratchpad files from issues/roadmaps/design docs
- ❌ Don't treat as deliverables or permanent documentation
- ❌ Don't include in commit messages or PR descriptions
- ⚠️ Mark clearly as "SCRATCHPAD - NOT DOCUMENTATION" in file header

**Cleanup**:
- Delete at end of session if no longer needed
- Human can delete entire `.scratchpad/` directory anytime
- Don't accumulate more than 2-3 scratchpad files at once

**Alternative: Operational Documentation**

If working notes would help users, **ask first**:
- "Should I create a setup guide for X?" (e.g., CI_CD_SETUP.md)
- "Would a troubleshooting doc for Y be helpful?"

**Operational docs are valuable** (CI setup, deployment guides, architecture overviews).  
**Session summaries are not** (redundant, stale instantly).

**Example Valid Operational Docs**:
- `docs/CI_CD_SETUP.md` - How to configure GitLab CI/CD
- `docs/DEPLOYMENT.md` - How to deploy to crates.io
- `docs/CONTRIBUTING.md` - How to contribute to the project

**Example Invalid Scratchpad Pollution**:
- `docs/project/SESSION_SUMMARY_2025-01-23.md` - ❌ Redundant
- `docs/project/AUTOMERGE_INTEGRATION_SUMMARY.md` - ❌ Info already in ISSUE_16
- `docs/project/PROGRESS_UPDATE.md` - ❌ Stale immediately

## Improving This Document

This document should evolve based on what works:

**After each session, consider:**
- Did the collaboration pattern work well?
- Were there repeated misunderstandings?
- Did document structure help or hinder?
- Did agent check existing code before proposing solutions?
- Were issues kept concise with references vs. full implementations?
- Did agent search for existing documents before creating new ones?
- Were issues under 300 lines?
- Did agent use scratchpad appropriately (vs creating summary docs)?

**Human should update AGENTS.md when:**
- A pattern emerges that should be formalized
- An anti-pattern is discovered
- Tool usage changes (new Zed features, etc.)
- Agent repeatedly wastes context in same way
- Session clarity improves after establishing new rule

**Version this document:**
Track meaningful changes to collaboration patterns, not just typos.

**Agent should suggest updates when:**
- Noticing repeated context waste patterns
- Discovering better ways to organize work
- Finding guidelines that conflict with practice
