# Agent Instructions

## 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- Think like an owl — slow, observant, and analytical.
- Examine this problem from multiple perspectives and identify the hidden factors most people overlook.
- Break the problem down more carefully and into smaller evaluatable tasks.
- Look at multiple angles instead of just one.
- Surface risks and tradeoffs.
- Call out things that aren’t immediately obvious.
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them – don't pick silently.
- Work Collaboratively with the user, ask questions, and seek clarification.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.
- Focus on correctness, maintainability, and performance in that order. 
- Identify the easy fix and the correct fix. Always choose correctness.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

---

**These guidelines are working if:** fewer unnecessary changes in diffs, fewer rewrites due to overcomplication, and clarifying questions come before implementation rather than after mistakes.

## VDS-MCP (Versioned Document Service)

VDS-MCP is a Model Context Protocol (MCP) server for managing Markdown documents as stable, versioned section trees.

**VDS 2.0** is production-ready and filesystem-authoritative: your Markdown files are the source of truth, and VDS
metadata (`.vds/`) stores stable IDs, version history, and snapshots as Git-friendly JSON files.

### Starting the Server

```json
{
  "mcpServers": {
    "vds": {
      "command": "vds-mcp",
      "args": ["--workspace", "/absolute/path/to/project", "serve"]
    }
  }
}
```

Use `serve` for the filesystem-authoritative VDS 2 mode.

### Key Operations

#### Document Management
- `list_documents` — list all managed documents in the workspace
- `get_document` — get document metadata and root section ID
- `create_document` — create a new Markdown file and manage it
- `manage_document_file` — adopt an existing Markdown file into VDS tracking
- `get_document_location` — get the workspace-relative path of a document
- `rename_document` — rename the document (file move + metadata update)
- `import_document` — adopt an existing file by path
- `export_document` — render a document back to a Markdown file
- `remove_document_file` — soft-delete a document (archived, restorable)
- `restore_document_file` — restore a soft-deleted document
- `unmanage_document_file` — stop tracking a file without deleting it

#### Section Operations
- `get_section` — retrieve a section's title, content, level, and version
- `get_section_tree` — get a section and all its descendants
- `table_of_contents` — get a document's heading outline
- `create_section` — add a new section (child of a parent, or sibling)
- `update_section` — replace a section's content
- `append_to_section` — add content to the end of a section
- `rename_section` — change a section's heading title
- `insert_section_before` / `insert_section_after` — insert relative to a sibling (use `sibling_section_id`)
- `move_section` — relocate a section to a different parent
- `reorder_sections` — reorder children under a parent (use `parent_id` and `ordered_children`)
- `promote_section` / `demote_section` — change heading level
- `remove_section` — remove a section; set `remove_children: true` to remove descendants too
- `split_section` — split at a byte offset (`split_at` field, not `split_content`)
- `set_section_metadata` — update anchor, tags, summary, or lock state

#### Version History
- `section_versions` — list all version IDs for a section
- `get_section_version` — retrieve a historical version by version ID
- `diff_section_versions` — compare two versions
- `switch_section_version` — restore a section to a prior version

#### Snapshots
- `create_document_snapshot` — save the full document tree state with an optional label
- `document_snapshots` — list all saved snapshots
- `diff_document_snapshots` — compare two snapshots
- `restore_document_snapshot` — revert the document to a snapshot state

#### Search
- `full_text_search` — BM25 lexical search across all section titles and content
  - Supports `"quoted phrases"`, `prefix*` queries, AND/OR modes, and path filters
  - camelCase/PascalCase identifiers are tokenized into sub-words automatically
- `semantic_search_sections` — nearest-neighbor semantic search (requires `--features semantic-search` build)
  - **Requires pre-computed embeddings** — pass embedding vector via `query_embedding` parameter
  - VDS caches embeddings by (section_id, content_hash, model) but does not generate them
  - Uses HNSW index for fast approximate nearest neighbors
- `find_by_title` — exact or fuzzy title matching
- `find_by_tag` — search by section metadata tags

#### Workspace
- `get_workspace` — current workspace path, watcher status, and reload count
- `set_workspace` — switch to a different workspace at runtime
- `validate_document` — check content hash, version files, and snapshot references

### Usage Tips

1. **Conflict detection**: Every mutation accepts an optional `expected_content_hash`. Supply the hash returned
   by `get_document_location` or `get_section` to detect external edits before they are silently overwritten.

2. **Safe edits**: Read the section first, note its `current_version`, then call the mutation tool. If another
   agent or a human edited the file between read and write, VDS returns `ExternalContentConflict`.

3. **Structural vs. content mutations**: `update_section`, `append_to_section`, `rename_section`, and
   `set_section_metadata` are surgical (fast, byte-range). `create_section`, `remove_section`, `reorder_sections`,
   `move_section`, `promote_section`, `demote_section`, and `split_section` re-render the whole file canonically.

4. **remove_section**: The `remove_children` field is required. Pass `false` to detach children (they become
   siblings of the removed section) or `true` to delete the section and all descendants.

5. **reorder_sections**: Use `parent_id` (not `parent_section_id`) and `ordered_children` (not `ordered_section_ids`).

6. **split_section**: Use `split_at` (byte offset into the section content) to control the split point.

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:970c3bf2 -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md for details and anti-patterns.

## Agent Context Profiles

The managed Beads block is task-tracking guidance, not permission to override repository, user, or orchestrator instructions.

- **Conservative (default)**: Use `bd` for task tracking. Do not run git commits, git pushes, or Dolt remote sync unless explicitly asked. At handoff, report changed files, validation, and suggested next commands.
- **Minimal**: Keep tool instruction files as pointers to `bd prime`; use the same conservative git policy unless active instructions say otherwise.
- **Team-maintainer**: Only when the repository explicitly opts in, agents may close beads, run quality gates, commit, and push as part of session close. A current "do not commit" or "do not push" instruction still wins.

## Session Completion

This protocol applies when ending a Beads implementation workflow. It is subordinate to explicit user, repository, and orchestrator instructions.

1. **File issues for remaining work** - Create beads for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **Handle git/sync by active profile**:
   ```bash
   # Conservative/minimal/default: report status and proposed commands; wait for approval.
   git status

   # Team-maintainer opt-in only, unless current instructions forbid it:
   git pull --rebase
   bd dolt push
   git push
   git status
   ```
5. **Hand off** - Summarize changes, validation, issue status, and any blocked sync/commit/push step

**Critical rules:**
- Explicit user or orchestrator instructions override this Beads block.
- Do not commit or push without clear authority from the active profile or the current user request.
- If a required sync or push is blocked, stop and report the exact command and error.
<!-- END BEADS INTEGRATION -->

<!-- SPECKIT START -->
For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan
<!-- SPECKIT END -->

## Imported Claude Cowork project instructions

Think like an owl — slow, observant and analytical. Examine this problem from multiple perspectives and identify the hidden factors most people overlook. Break the problem down more carefully, Look at multiple angles instead of just one, Surface risks and tradeoffs, Call out things that aren’t immediately obvious. Work Collaboratively with the user, ask questions, and seek clarification. Analyze your own answers for correctness and clairity. Stay focused on the topic at hand.
