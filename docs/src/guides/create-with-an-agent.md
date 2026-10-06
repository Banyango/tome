# Create workflows with an agent

Don't write workflows by hand. Describe what you want to your coding agent, and let it write the file with the [tome plugin](../agents/skill.md).

A workflow's body is instructions for another agent, so an agent is good at writing it. Its skill knows the frontmatter, the trigger fields, how workers, queues and worktrees fit together, and the mistakes that make a run stall or loop. It asks you the questions a workflow depends on, then writes the file and runs `tome validate` on it. You review the result and run it.

The rest of the guides are reference for what the agent writes. Read them when you want to check its work or understand a run, not before you start.

## Set up

1. [Install tome](../install.md) and start the daemon with `tome daemon start`.
2. [Install the tome plugin](../agents/skill.md) in Claude Code:

   ```text
   /plugin marketplace add banyango/tome
   /plugin install tome@tome
   ```

   [Install tome (for agents)](../agents/install.md) does both steps for you. Other agents can use the skill on its own.
3. Run `/reload-plugins`, or start a new session in your project, so it loads.

## Ask for a workflow

Say what should happen and what starts it. You don't need tome's terms. For example:

```text
Write a tome workflow that runs the tests whenever I change a file under
src/, and has an agent fix anything that fails.
```

```text
Every weekday at 9am, list branches with no commits in two weeks and write
them to stale-branches.md.
```

```text
When I add features/<slug>/feature.md, have an agent read the code, break
the feature into tasks, and write them next to it. Then build each task on
its own branch.
```

```text
Take the "Review a branch in parallel" recipe from the tome docs and adapt it
to this repo.
```

Starting from a [recipe](../recipes.md) works well: the agent changes the test commands, paths and branch names to fit your project.

## Answer its questions

Before it writes anything, the agent asks about what the request and the repo don't already say:

- what starts the workflow: you, a file change, a schedule or a message
- how many agents may run at once. Each is a full agent session, and most laptops handle 1 or 2. Start with 1.
- how the work is split: one agent for everything, one per item, or a few taking items from a queue
- where changes go: the current branch, or one branch per item, and whether they're merged or left for review
- when it stops, and which commands check the work
- what to call the workers and their branches

It suggests an answer for each, so you can accept most of them. Then it says back in a few lines what the workflow will do. If that's not what you meant, say so now: it's cheaper than a wrong run.

Your answers become params with defaults, such as `workers: {type: int, default: 1}`, so the next run needs no questions and you can change them with `--param`.

## Review what it wrote

The agent writes `.tome/workflows/<name>.md` and runs `tome validate` on it. Read the file before the first run. It's short, and most of it is plain English. Check that:

- the trigger matches what you asked for. A file trigger on files the run itself edits starts another run each time.
- the body never has more workers running than you said
- each step says what done looks like and what happens if it fails
- the commands it runs are the ones your project uses

Ask the agent to fix anything that's off, rather than editing the file yourself. It re-validates after each change.

## Try it small

Have the agent start a small first run, such as a single item, and watch it:

```sh
tome run my-workflow --param limit=1
tome runs show <id>
```

For a triggered workflow, fire the trigger by hand instead of waiting for it:

```sh
tome triggers fire my-workflow --dry-run
tome triggers fire my-workflow --path src/lib.rs
```

Every agent runs in a tmux, cmux or herdr session you can watch and steer.

## Change it

When a run does something you didn't want, tell the agent what happened and what you want instead:

```text
The tome run 12 of fix-tests started a second run when it edited src/. Stop
that.
```

```text
Let fix-tests run two workers at once, and open a PR instead of merging.
```

The agent can read the run's steps and logs with `tome runs show` and `tome runs logs`, so give it the run id. It changes the workflow, validates it, and tells you what changed. Commit the file when you're happy with it, so the workflow runs the same way for everyone on the project.

## When to edit by hand

Small, obvious changes are fine to make yourself: a default, a cron time, a path in a trigger. Run `tome validate <name>` afterwards. For anything that changes how the work is split or when the workflow starts, ask the agent.

If you don't use a coding agent, [Write a workflow](write-a-workflow.md) has every frontmatter option and [Recipes](../recipes.md) has files to copy.
