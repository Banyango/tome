# Overview

**Tome** is a CLI for managing agentic workflows. You use it to define [[workflow]]s and run them.

_(source: intent.md)_

## Problem

Agentic workflows are hard to orchestrate. Repeating a workflow turns into a mess of markdown files, and you are stuck with a single agent harness. Terminal multiplexers work well for orchestrating a workflow, watching it run, and responding to problems as they come up. However, a workflow set up in one session is hard to copy to a new project or to change slightly.

_(source: intent.md)_

## Solution

Tome is a CLI that agents use to coordinate their work. Terminal multiplexers (cmux, tmux and herdr) are the UI layer, and tome coordinates them through a pluggable [[backend]]. A skill lets users specify workflows by describing the steps a workflow should take. Workflows can run agents, open new multiplexer windows, run CLI commands, notify the user, and more.

Tome provides [[primitive]] objects ([[queue]], [[session]], [[worktree]], [[group]]), [[trigger]]s (a file is created or edited, a group of tasks completes, a message arrives) and [[action]]s (fan-out, fan-in, and others).

_(source: intent.md)_

## Who uses it

- **Agents** are the main day-to-day callers. They invoke `tome` commands while a workflow is running.
- **Humans** write [[workflow]] definitions, mainly through the skill, and respond to [[notification]]s.

_(source: .sengify/sources/interview-2026-09-26.md)_

## Key design decisions

- **Runtime:** a long-running [[daemon]] watches for [[trigger]]s and runs [[action]]s. CLI commands talk to it.
- **Workflow format:** markdown with structured frontmatter. Workflows load from `~/.tome/workflows` (global) and `.tome/workflows` (project), and a project workflow overrides a global one with the same name. This handles the "copy to a new project" problem.
- **Harness independence:** agents launch through a generic [[harness-adapter]] (a configurable command template), so tome isn't tied to one harness.
- **Data between steps:** files on disk and git [[worktree]]s.
- **Step completion:** a [[step]] finishes when the agent calls tome, the process exits, a timeout fires, or a human approves.
- **Failure handling:** each workflow can define its own failure conditions. By default, tome sends the user a [[notification]].
- **v1 scope:** all three backends (cmux, tmux, herdr) are supported.

_(source: .sengify/sources/interview-2026-09-26.md)_

## Technical details

- Language: Rust
- Interface: CLI
- Storage: an embedded database holding [[run]] state. See the contradiction below.

_(source: intent.md; .sengify/sources/interview-2026-09-26.md)_

> **⚠ Contradiction (flagged):** `intent.md` says "SQLLite as an initial implementation." The 2026-09-26 interview says to **use DuckDB instead**, and that it holds run state only. This wiki follows the interview decision. `intent.md` has not been changed, so update it if DuckDB is final.
>
> **✓ Resolved 2026-09-26:** `intent.md` now says DuckDB, which matches the interview.

## Planned features

Initial feature set, in dependency order:

- **Foundation:** workflow definition format ([[workflow]]); daemon lifecycle and CLI↔daemon protocol ([[daemon]]); run state storage in DuckDB ([[run]])
- **Execution core:** running a workflow (`tome run`); step completion and failure handling ([[step]]); the harness adapter ([[harness-adapter]])
- **Primitives:** terminal backends ([[backend]]); [[queue]]s; [[worktree]] management; [[group]]s with fan-out and fan-in
- **Triggers and I/O:** [[trigger]]s; [[notification]]s and human approval; the send-message [[action]]
- **Authoring:** the workflow authoring skill

Start with the workflow definition format and running a workflow, because nearly every other feature depends on them.

_(source: .sengify/sources/feature-set-2026-09-26.md)_
