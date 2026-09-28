# File triggers via OS file events — Task Plan

Source feature: [feature.md](./feature.md)

> All tasks are done: implemented as 005-1 to 005-5 and merged into main in eee3a9b.

## Tasks

### 005-1. OS-event file watching as the default

**Blocked by:** none

Armed file triggers get their changes from OS file events through the `notify` crate, not from the daemon walking every watched folder on each tick. Only the directories the polling walk would visit are watched. That is everything under the glob's literal base, excluding `.git/`, `.tome/`, gitignored dirs and the trigger's `ignore:` globs, and this ignore set is fixed at arm time. On Linux each directory gets its own inotify watch, and new directories get one as they appear. On macOS one FSEvents stream covers the base, and ignored paths are filtered out of its events. Each trigger builds a file-stat index when it arms, without firing, and the index decides created vs modified. Files already inside a directory when it appears, such as a moved-in dir, count as `created`, and a file moved or renamed into the glob counts as `created` too. Deletes never fire. Changes while the workflow's run is active are dropped (muted), but they still update the index so they don't fire later. Everything a workflow author can see stays exactly as in 004-triggers: trigger syntax, `on:`, the 2s debounce batching, muting, shadowing and path forms.

### 005-2. Lossless rescans when the OS drops events

**Blocked by:** 005-1

When the OS says events were lost (an inotify queue overflow or an FSEvents MustScanSubDirs), the trigger walks its watched tree once and diffs it against the index. The resulting created and modified changes go through the normal debounce and `on:` filtering, so a burst like a big checkout still fires correctly and no change is lost.

### 005-3. Missing or deleted glob base dir

**Blocked by:** 005-1

When a file trigger's glob base dir doesn't exist yet, or is deleted, the trigger watches the nearest existing ancestor. When the base dir appears, its existing files are indexed without firing. Files created after that fire as usual.

### 005-4. Polling fallback with visibility

**Blocked by:** 005-1

A trigger that can't get an OS watch falls back to the existing polling watcher and keeps working. Causes include the inotify watch limit, an unsupported filesystem such as a network drive, and a permission error. The fallback and its reason are logged in `daemon.log`, and `tome triggers ls` marks the trigger as polling with that reason. No notification is sent, because the trigger still works. The trigger stays on polling until it is re-armed: its workflow changes, triggers are disabled and re-enabled, or the daemon restarts. There are no periodic retries.

### 005-5. `TOME_FILE_WATCH=poll` override

**Blocked by:** 005-4

Setting `TOME_FILE_WATCH=poll` forces every file trigger onto polling, in the same style as `TOME_TRIGGER_TICK_MS` and `TOME_NOTIFY`. `tome triggers ls` shows these triggers as polling because it was forced. Users can use it to debug or to work around a filesystem the OS watcher misreports, and it lets both watch paths be tested. An unknown value logs a warning and falls back to OS events.
