# Agent Scratchpad Directory

**Purpose**: Ephemeral working notes for AI agents during complex sessions.

## What This Is

This directory contains **temporary working files** that agents (like Claude) use to:
- Organize scattered context into coherent architecture
- Check consistency across multiple issues/documents
- Validate cross-references and ensure documents link properly
- Plan complex architectural decisions

## What This Is NOT

- ❌ **Not documentation** - Users should never need to read these files
- ❌ **Not deliverables** - Don't reference from issues/roadmaps/design docs
- ❌ **Not permanent** - Delete files at end of session or when no longer needed
- ❌ **Not in git history** - Don't include in commit messages

## Rules

1. **Agent Permission**: Agents can create/read/write files here without explicit authorization
2. **Clear Marking**: Every file MUST have "SCRATCHPAD - NOT DOCUMENTATION" in header
3. **Cleanup**: Delete files when no longer needed; don't accumulate more than 2-3 at once
4. **No References**: Never link to scratchpad files from permanent documentation

## When to Use Scratchpad vs. Creating Docs

### Use Scratchpad For:
- Organizing your own thoughts during a session
- Checking if Issue X properly references Issue Y
- Planning a complex refactoring across multiple files
- Validating that architecture decisions are consistent

### Ask User Before Creating:
- Operational guides (CI setup, deployment, troubleshooting)
- Architecture overviews for users
- Contributing guidelines
- Setup instructions

## Cleanup

**Human**: You can delete this entire directory anytime. It won't break anything.

**Agent**: Delete files at end of session. If you find old scratchpad files from previous sessions, delete them.

## Example Valid Usage

```
.scratchpad/
├── README.md (this file)
├── 2025-01-23-event-log-planning.md (working notes for ISSUE_16)
└── cross-reference-check.md (temporary validation notes)
```

After session completes: Delete `2025-01-23-event-log-planning.md` and `cross-reference-check.md`.

---

**Last Updated**: 2025-01-23  
**See**: `AGENTS.md` - Agent Scratchpad section for full guidelines