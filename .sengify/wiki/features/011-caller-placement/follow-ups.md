# Orchestrator next to the caller — Follow-ups

Source feature: [feature.md](./feature.md) · Tasks and what changed: [tasks.md](./tasks.md)

Gaps and rough edges found while implementing 011-1 to 011-4 (2026-09-28). None of them block the feature; each is a candidate for a later task.

- **`TOME_LAYOUT` can't say `from: caller`:** it carries only a layout name. Giving it a preset name would let a shell default to opening next to the caller.
- **A later move drops `from: caller`:** a move without `--from caller` loses it. A `--layout tab` move then leaves the caller's workspace for the resolved one, which may surprise someone who only wanted a tab instead of a split.
- **Focus isn't checked by a test:** the tab and split open with `--focus false`, and the tests check where the surface lands, not that the chat kept focus.
- **The caller's window is ignored:** if the caller's workspace is in another cmux window, the orchestrator goes there too. Nothing checks this or warns about it.
- **The run's `caller` reason is only shown when used:** a run whose caller is unknown doesn't say why in `tome runs show` unless a session tried `from: caller`.
- **cmux `split.size` next to the caller:** it's best effort, as in 010. The caller's workspace is usually on screen, so it should normally stick, but that isn't tested.
