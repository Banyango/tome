# Workflows

Workflow files: markdown with YAML frontmatter and a natural-English body. This
feature parses and validates them and implements the `tome workflow` and
`tome validate` commands.

## Files

- `src/workflow.rs`: `Workflow`, `Frontmatter`, `Defaults`, `ParamSpec`, `Trigger`
  and its kinds, `Scope`, `Library` (what workflows are visible), `parse`,
  `parse_snapshot`, `check_scope`, `Invalid`/`Diagnostic`. The largest file;
  parsing, validation, library lookup and param-arg parsing all live here.
- `src/workflowcmd.rs`: the only place workflow errors meet the CLI: `From`
  conversions from `Invalid`, `LibraryError`, `FindError` and `ParamArgError` to
  `CliError` (exit kind, hint, JSON details).
- `src/validate.rs`: `tome validate [workflow] [--param k=v]`. Local, no daemon.
- `src/scaffold.rs`: `tome workflow new|rm|ls`.
- `src/cron.rs`, `src/duration.rs`, `src/glob.rs`: small pure parsers used by
  frontmatter (`cron:`, durations, file-trigger globs).
- `src/topic.rs`: topic names and `on:` patterns (shared with the bus).

## Notes

- Workflows resolve in two scopes: project (`.tome/workflows`) and global
  (`~/.tome/workflows`); `check_scope` enforces what each may declare.
- A run stores a **snapshot** of its workflow (`parse_snapshot`), so edits don't
  change running or resumed runs.
- `workflow.rs` has no dependency on `output`: it returns its own errors
  (`Invalid`, `LibraryError`, `FindError`, `ParamArgError`). Add new CLI wording,
  hints or exit kinds in `workflowcmd.rs`, not in `workflow.rs`.
- `Library` still does filesystem and plugin-store reads (`discover`, `entries`);
  that is the next thing to move out of the parsing code.

## Tests

- Unit: `cargo test --bin tome workflow::` (also `cron::`, `duration::`, `glob::`, `topic::`)
- Integration: `validate`, `workflow_ls`, `workflow_new`, `workflow_rm`, `project_config`

## User docs

`docs/src/concepts/workflows.md`
