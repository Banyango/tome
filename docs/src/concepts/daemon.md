# The daemon

The **daemon** is a long-running tome process. It:

- starts runs, and launches each run's orchestrator,
- records the step progress the orchestrator reports,
- watches for [triggers](triggers.md) (file changes, cron times, messages) and starts or signals runs,
- creates and watches the sessions that agents run in,
- recovers when it restarts: runs that were in progress are failed.

The daemon does not decide what a workflow does. It doesn't decide step order either. The orchestrator does that.

Every `tome` command that touches runs talks to the daemon over a local socket in `~/.tome`. If the daemon isn't running, those commands stop with an error that says how to start it.

```sh
tome daemon start     # in the background
tome daemon status    # exit code 3 if it isn't running
tome daemon stop
tome daemon install   # start at login (launchd on macOS, systemd --user on Linux)
```

`tome start` and `tome stop` are shorthands for `tome daemon start` and `tome daemon stop`.

A daemon that crashes is restarted by the login service. `tome daemon stop` is a clean exit, so it stays stopped.

With cmux, start the daemon from a terminal inside cmux. cmux only lets its own processes control it, and the daemon inherits that from where it started. See [Backends](backends.md).

See [Operating tome](../guides/operating.md) for more.
