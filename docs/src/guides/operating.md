# Operating tome

## The daemon

```sh
tome daemon start      # in the background
tome daemon status     # prints pid, version and uptime; exit code 3 if it isn't running
tome daemon stop
tome daemon run        # in the foreground, for debugging or a service manager
```

`tome start` and `tome stop` are shorthands for `tome daemon start` and `tome daemon stop`.

To keep it running across logins and reboots, register it as a service:

```sh
tome daemon install     # launchd on macOS, systemd --user on Linux
tome daemon uninstall
```

The service restarts the daemon if it crashes. `tome daemon stop` is a clean exit, so it stays stopped.

If the daemon restarts, runs that were in progress fail with the reason `daemon_restart`, since nothing was watching them.

State and logs are in `~/.tome`: the DuckDB database, the daemon's log (`daemon.log`) and each run's logs (`runs/<id>/`). Set `TOME_HOME` to use another directory.

With cmux, start the daemon from a terminal inside cmux; see [Sessions and backends](sessions.md).

## Starting and stopping runs

```sh
tome run my-workflow --param key=value   # attached: streams progress, Ctrl-C cancels
tome run my-workflow --detach            # print the run id and return
tome run my-workflow --harness codex --model <model-name>
tome run cancel <id>                     # kill its sessions; worktrees are kept
```

`--harness` and `--model` override the workflow's choices for the main agent and workers, including the orchestrator. Configure the harness first; see [Sessions and backends](sessions.md#harnesses).

## Looking at runs

```sh
tome runs list                            # newest first
tome runs list --status running
tome runs list --workflow my-workflow --limit 50
tome runs show <id>                       # status, steps, history, workers, worktrees, sessions
tome runs show <id> --snapshot            # the workflow as it was when the run started
tome runs logs <id> --tail 100            # each session's log
tome runs logs <id> --step Review
```

Add `--json` to any command for machine-readable output. Setting `TOME_OUTPUT=json` does it for every command.

## Querying

`tome query` runs one read-only SQL statement against the run database:

```sh
tome query "select id, workflow_name, status, reason from runs order by id desc limit 5"
tome query "select run_id, name, status, summary from workers where status = 'failed'"
tome query "describe runs"
```

The tables are `runs`, `steps`, `step_events`, `workers`, `worker_events`, `worker_groups`, `worktrees`, `sessions`, `queues`, `queue_messages`, `logs`, `bus_events`, `deliveries`, `trigger_fires` and `projects`. Only `select`, `with`, `describe` and similar statements are allowed.

## Cleaning up

Nothing is deleted on its own. `tome gc` deletes finished runs older than an age, with their logs and worktrees:

```sh
tome gc --older-than 14d --dry-run    # show what would go
tome gc --older-than 14d
```

Ages look like `12h`, `7d` or `2w`. A worker's branch is deleted only if it was merged into its base. Unmerged branches are kept, and gc lists them. Bus events are cleared once all their deliveries are settled.

## Removing workflows

```sh
tome workflow rm my-workflow            # the project's workflow
tome workflow rm my-workflow --global   # the one in ~/.tome/workflows
```

There's no prompt. The file is deleted, and runs and their logs stay. The command refuses while a run of the workflow is running or queued. `--force` overrides that, and is also how to remove one while the daemon is down.

## Triggers

`tome triggers ls` shows armed triggers, and `tome triggers disable` and `enable` pause and resume a project's. See [Triggers and the message bus](triggers.md).

## Troubleshooting

**"tome daemon is not running".** Run `tome daemon start`.

**A run fails with `orchestrator_exited` or `worker_exited`.** The agent's session ended without reporting. Read its log with `tome runs logs <id>`. Common causes are a harness that can't start (a wrong command, or not logged in) and an agent that was closed by hand.

**A run fails because an agent "never made a first tome call".** The agent didn't start in time. tome nudges it once, then fails it. If your harness is slow to start, raise `defaults.start_timeout` in the workflow, or set it to `off`.

**Sessions don't open in cmux.** The daemon has to be started from inside a cmux terminal. Stop it and start it again from there.

**A harness won't start or refuses a model.** Run `tome harness validate <name> --model <model-name>` to check configuration, executable availability and model forwarding without launching the agent. Omit `--model` when none is selected. For shell templates, check the agent executable separately; validation checks `sh` and shell syntax. Complete the agent CLI's sign-in, and make sure the daemon's `PATH` includes it.

**A workflow isn't found.** `tome validate` lists every workflow tome can see. Project workflows are looked up from the current directory upwards, so run commands from inside the project.

**`tome validate` fails.** It names the file and line. A frontmatter key it doesn't know, a `{{placeholder}}` that names no param, and a param without a default that a trigger doesn't fill are the usual causes.

**A trigger isn't firing.** Check that the daemon is running and that `tome triggers ls` shows the trigger, then try `tome triggers fire <workflow> --dry-run`.

**A file trigger keeps starting runs.** The workflow is probably editing files its own trigger watches. Add `ignore:`, `on: [created]` or `while_running: mute`.

**Something is stuck in the bus.** `tome events ls` shows backlogs, and `tome events retry` or `remove` fixes a delivery.

For every command and flag, run `tome --help` or `tome <command> --help`.
