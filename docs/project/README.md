# Project Management and Issue Resolution Workflow

This directory contains issues, roadmaps, and planning documents for `noet-core` development. This README documents the collaborative workflow used between human developers and AI agents to manage work effectively.

## Directory Structure

```
docs/project/
├── README.md                    # This file - workflow documentation
├── BACKLOG.md                   # Optional enhancements, extracted from completed issues
├── ROADMAP*.md                  # Version-specific and feature-specific roadmaps
├── ISSUE_*.md                   # Active issues (in progress or planned)
├── completed/                   # Completed and resolved issues
│   ├── ISSUE_*.md              # Archived completed work
└── trades/                      # Trade studies (architectural alternatives analysis)
    └── *.md                     # Decision documents for complex choices
```

## Issue Resolution Process

Our workflow follows a structured cycle that enables efficient collaboration between humans and AI agents while maintaining high quality and clear documentation.

### 1. Issue Creation and Planning

**Create the Issue Document** (`ISSUE_XX_TITLE.md`):
- Use sequential numbering (never reuse numbers, even for deleted issues)
- Search for existing issues first: `find_path "ISSUE_*"`
- Follow the issue template structure (see AGENTS.md § Issues and Roadmaps)
- Target length: 150-250 lines (300 lines maximum)

**Define the Issue**:
- **Summary**: 2-3 sentences explaining the problem and outcome
- **Goals**: 3-5 specific, measurable objectives
- **Architecture**: High-level approach (diagrams, data structures, key decisions)
- **Implementation Steps**: Numbered steps with effort estimates
- **Testing Requirements**: Critical test scenarios
- **Success Criteria**: Measurable "done" conditions
- **Risks**: Known risks with mitigation strategies

**Assign Priority and Map to Roadmap**:
- Priority: CRITICAL | HIGH | MEDIUM | LOW
- Link to relevant roadmap (e.g., `ROADMAP_v0.1.md`)
- Identify dependencies (requires/blocks other issues)
- Estimate effort (for relative comparison, not commitments)

### 2. Initial Investigation and TDD Scaffolding

**Understand the Problem Space**:
- Review existing code using `grep` for related functionality
- Read relevant design documents in `docs/design/*.md`
- Check module-level rustdoc for API patterns
- Identify existing infrastructure to extend (don't reinvent)

**Test-Driven Development Scaffolding**:
- Write failing tests first (capture requirements)
- Create test fixtures and data
- Document expected behavior through test assertions
- Establish baseline: `cargo test` shows current state

**Investigation Outcomes**:
- Update issue with findings (Architecture section)
- Identify unknowns and create trade studies if needed
- Refine implementation steps based on discoveries
- Adjust effort estimate if significantly off

### 3. Context Budget Check

**Before Starting Implementation**:
- Check current token usage (aim to stay under 50% budget for implementation)
- Assess remaining work complexity
- Decide: Implement now or defer to new session?

**If Implementing Now**:
- Proceed to Step 4

**If Deferring**:
- Update issue with investigation findings
- Document next steps in "Implementation Notes" section
- Note session progress in scratchpad (`.scratchpad/`)
- Communicate clear handoff: "What got done, what's next, open questions"

### 4. Implementation and Testing

**Implement the Solution**:
- Follow implementation steps from issue
- Make code changes incrementally
- Run tests frequently: `cargo test`
- Add logging/tracing for debuggability
- Keep changes focused on issue scope

**Test as You Go**:
- Verify each implementation step with tests
- Fix failing tests immediately if simple (1-2 line fixes)
- HALT if tests require investigation or multiple changes (summarize state, let human decide)
- Don't get caught in fix-failure loops

**Handle Test Failures**:
- Simple fix (obvious one-liner)? → Fix it
- Requires investigation? → HALT, summarize, defer to human
- Multiple speculative changes? → HALT, summarize current state
- Never run `git revert` or destructive git commands

### 5. Update the Issue

**Document What Was Completed**:
- Add "Implementation Status" or "Resolution" section
- Mark completed checkboxes with `[x]`
- Note files modified
- Document design decisions made
- Include examples of functionality

**Update Success Criteria**:
- Check off completed criteria: `- [x]`
- Leave incomplete criteria unchecked: `- [ ]`
- Add notes if partial completion

**Capture Learnings**:
- Document bugs discovered and fixed
- Note architectural insights
- Record performance characteristics
- Update "Lessons Learned" subsection

### 6. Identify Unresolved Items

**Review Unchecked Boxes**:
- Are they truly incomplete or just planning notes?
- Do they represent real work that needs tracking?
- Which are blockers vs. nice-to-haves?

**Categorize Remaining Work**:

**Large/Important Items** → Create New Issue:
- Substantial scope (>0.5 days effort)
- Blocks other work or v1.0 release
- Requires architectural decisions
- Example: "Implement database sync" discovered during file watching work

**Small/Optional Items** → Move to BACKLOG.md:
- Nice-to-have enhancements
- Performance optimizations
- Documentation improvements
- Example: "Add BID collision detection warnings"

**Out of Scope** → Document as "Not Implemented":
- Explicitly decided against
- Deferred to post-1.0
- Document rationale in issue

### 7. Decide if Issue is Complete

**Check for Orphaned Actions**:
- Are all critical items resolved or tracked elsewhere?
- Do all unchecked boxes have a destination (new issue/backlog/deferred)?
- Are success criteria met or documented as incomplete?

**If Orphaned Actions Remain**:
- Continue implementation (if in scope and budget allows)
- Create new issues for large items
- Move small items to backlog
- Document deferred work with rationale

**If No Orphaned Actions**:
- Proceed to Step 8 (Move to Completed)

### 8. Move to Completed Folder

**Mark as Complete**:
- Add "✅ COMPLETE" to title: `# Issue XX: Title - ✅ COMPLETE`
- Add completion date: `**Status**: COMPLETE (YYYY-MM-DD)`
- Add actual effort if different from estimate: `**Estimated Effort**: 3-5 days (Actual: 0.5 days)`
- Mark all relevant success criteria as complete: `- [x]`

**Final Documentation Pass**:
- Ensure "Resolution" or "Implementation Status" section is comprehensive
- Include test results
- Note files modified
- Document lessons learned
- Update cross-references to other issues

**Move to Completed**:
```bash
mv docs/project/ISSUE_XX_*.md docs/project/completed/
```

**Update References**:
- Update roadmaps to mark issue as complete
- Update BACKLOG.md if items were extracted
- Update cross-references in other issues

## Issue Templates and Examples

### Minimal Issue Template

```markdown
# Issue N: [Title]

**Priority**: CRITICAL | HIGH | MEDIUM | LOW
**Estimated Effort**: N days
**Dependencies**: Requires Issue X, Blocks Issue Y

## Summary
2-3 sentences: Problem and outcome

## Goals
- Goal 1
- Goal 2
- Goal 3

## Architecture
High-level approach, data structures, key decisions

## Implementation Steps
1. Step name (effort)
   - [ ] Task
   - [ ] Task

## Testing Requirements
- Critical test scenarios

## Success Criteria
- [ ] Measurable outcome 1
- [ ] Measurable outcome 2

## Risks
- Risk: Description
  **Mitigation**: Solution
```

### Completed Issue Example

See `completed/ISSUE_23_INTEGRATION_TEST_CONVERGENCE.md` for a well-documented completed issue.

## Workflow Principles

### Test-Driven Development
- Write tests first (capture requirements as assertions)
- Tests are specifications (show expected behavior)
- Failing tests drive implementation
- Passing tests confirm completion

### Incremental Progress
- Small, focused changes
- Commit frequently
- Test after each change
- Document as you go

### Clear Handoffs
- Update issue before ending session
- Note what's done, what's next, open questions
- Use scratchpad for session notes (`.scratchpad/`)
- Don't leave work in ambiguous state

### Context Efficiency
- Read file outlines before full content
- Use targeted `grep` searches
- Reference existing docs instead of duplicating
- Keep issues under 300 lines

### Quality Standards
- All tests passing before marking complete
- No ignored tests without justification
- Code reviewed (by human or documented)
- Documentation updated

## Common Patterns

### Investigation-Heavy Issues
1. Create issue with known unknowns
2. Investigation phase updates "Architecture" section
3. May spawn trade study document
4. Implementation steps refined after investigation

### Multi-Session Issues
1. Session 1: Investigation and scaffolding
2. Update issue with findings, defer implementation
3. Session 2: Implementation using refined plan
4. Session 3: Polish and completion

### Emergent Issues
- Discovered during other work
- Quick capture: minimal issue with summary and goals
- Flesh out architecture during investigation
- Prioritize relative to existing work

## Anti-Patterns to Avoid

### ❌ Incomplete Issues in Active Directory
- **Problem**: Stale issues clutter workspace
- **Solution**: Move to completed or archive, track remaining work elsewhere

### ❌ Unchecked Boxes Without Tracking
- **Problem**: Lost work, unclear status
- **Solution**: Explicitly categorize all incomplete items (new issue/backlog/deferred)

### ❌ Implementation Without Tests
- **Problem**: Unverified behavior, regressions
- **Solution**: TDD scaffolding before implementation

### ❌ Design Decisions Not Documented
- **Problem**: Rationale lost, decisions revisited
- **Solution**: "Design Decisions Made" section in issue

### ❌ Issues > 300 Lines
- **Problem**: Hard to review, signal diluted
- **Solution**: Extract to design docs or trade studies

## Integration with AGENTS.md

This workflow is designed for human-AI collaboration. See `AGENTS.md` for:
- Issue structure guidelines (§ Issues and Roadmaps)
- Document length targets (§ Issue Length Target)
- When to split issues (§ Recognizing When to Split Issues)
- Session management (§ Session Management)
- Scratchpad usage (§ Agent Scratchpad)

## Tools and Commands

### Finding Issues
```bash
# List all active issues
find_path "ISSUE_*"

# List completed issues
find_path "completed/ISSUE_*"

# Search issue content
grep "keyword" "docs/project/ISSUE_*.md"
```

### Creating Issues
```bash
# Check highest issue number
ls docs/project/ISSUE_* | sort -V | tail -1

# Next issue number is highest + 1
```

### Moving to Completed
```bash
# After marking complete in document
mv docs/project/ISSUE_XX_*.md docs/project/completed/
```

## Maintenance

### Regular Review
- **Weekly**: Review active issues, update status
- **Monthly**: Review backlog, prioritize or archive
- **Per release**: Review roadmaps, mark completed work

### Cleanup
- Archive completed issues older than 6 months (optional)
- Remove stale scratchpad files
- Consolidate related backlog items

## Questions?

- See `AGENTS.md` for collaboration guidelines
- See `../design/` for architectural documentation
- See `BACKLOG.md` for optional enhancements
- See `ROADMAP*.md` for release planning

---

**Last Updated**: 2026-01-28
**Version**: 1.0 - Initial workflow documentation