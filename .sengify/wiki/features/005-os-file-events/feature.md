# File triggers via OS file events

## Description

File [[trigger]]s get their changes from OS file events (FSEvents on macOS, inotify on Linux, through the `notify` crate) instead of the [[daemon]] walking every watched folder on each tick. Everything a [[workflow]] author can see stays exactly as in 004-triggers:

- the trigger syntax and the meaning of `on:`;
- the 2s debounce batching, muting and shadowing;
- paths relative to the project root (absolute for global workflows);
- deletes never fire;
- a file moved or renamed into the glob counts as `created`.

The existing polling watcher stays only as a fallback, used when a trigger can't get an OS watch.

## Use Cases

1. As a workflow author, I want file triggers to react as soon as a file changes, without a per-second walk, so that large projects don't cost the daemon constant CPU.
2. As a workflow author, I want my existing file triggers to behave exactly as before, so that switching to OS events needs no workflow changes.
3. As a user, I want a trigger that can't get an OS watch to keep working by polling, so that network drives or a hit inotify limit don't silently break it.
4. As a user, I want `tome triggers ls` to show which triggers are polling and why, so that I can spot and fix degraded watches.
5. As a user, I want no change lost when the OS drops events, so that a burst like a big checkout still fires correctly.
6. As a user or developer, I want `TOME_FILE_WATCH=poll` to force polling, so that I can debug, work around a filesystem the OS watcher misreports, and test both paths.

## Constraints

- **Dependency:** the only new dependency is the `notify` crate.
- **Default and override:** OS events are the default. `TOME_FILE_WATCH=poll` forces polling for every trigger, in the same style as `TOME_TRIGGER_TICK_MS` and `TOME_NOTIFY`.
- **What gets watched:** only the directories the polling walk would visit. That is everything under the glob's literal base, excluding `.git/`, `.tome/`, gitignored dirs and the trigger's `ignore:` globs.
  - On Linux, directories are watched one by one, and new directories get watches as they appear. Gitignored trees like `node_modules/` and `target/` therefore don't use up inotify watches.
  - On macOS, one FSEvents stream covers the base, and ignored paths are filtered out of its events.
- **File-stat index:** each trigger keeps an index of file stats, built when it arms. The index decides created vs modified (FSEvents merges those flags) and is the baseline for rescans. Arming fires nothing, as before.
- **Fallback visibility:** a trigger that falls back to polling is logged in `daemon.log` and marked as polling in `tome triggers ls`, with the reason. There is no [[notification]], because the trigger still works.
- **Fallback recovery:** a trigger stays on polling until it is re-armed (its workflow changes, triggers are disabled and re-enabled, or the daemon restarts). There are no periodic retries.
- **Ignore set:** the set of ignored paths is fixed when the trigger arms. A `.gitignore` edit takes effect only on re-arm.

## Edge Cases

- **The OS asks for a rescan** (inotify queue overflow, FSEvents MustScanSubDirs) → walk the watched tree once and diff it against the index, so no change is lost.
- **A watch can't be set up** (inotify limit, unsupported filesystem, permission error) → that trigger falls back to polling, which is logged and shown in `tome triggers ls`.
- **The glob's base dir doesn't exist yet, or is deleted** → watch the nearest existing ancestor. When the base dir appears, index its files without firing. Files created after that fire as usual.
- **A dir is created inside the watched tree** → it gets a watch. Files already in it when it appears (such as a moved-in dir) count as `created`.
- **Changes while the workflow's run is active** → dropped (muted), but the index is updated so they don't fire later.
- **`TOME_FILE_WATCH` has an unknown value** → log a warning and use OS events.

## Related Entities

- [[trigger]]
- [[daemon]]
- [[workflow]]
- [[notification]]
