# Workflows

Workflow files: markdown with YAML frontmatter and a natural-English body. This
feature parses and validates them and implements the `tome workflow` and
`tome validate` commands.

## Files

- `src/workflow.rs`: `Workflow`, `Frontmatter`, `Defaults`, `ParamSpec`, `Trigger`
  and its kinds, `Scope`, `Library` (what workflows are visible), `parse`,
  `parse_snapshot`, `check_scope`, `Invalid`/`Diagnostic`. The largest file;
  parsing, validation, library lookup and param-arg parsing all live here.
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
- `parse_param_args` returns `CliResult`, one place where domain code touches the
  CLI error type.

## Tests

- Unit: `cargo test --bin tome workflow::` (also `cron::`, `duration::`, `glob::`, `topic::`)
- Integration: `validate`, `workflow_ls`, `workflow_new`, `workflow_rm`, `project_config`

## User docs

`docs/src/concepts/workflows.md`
