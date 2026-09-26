# Interview — 2026-09-26

Clarifying interview on `intent.md`, conducted before the initial wiki ingest. Answers are the user's.

| Topic | Question | Answer |
|---|---|---|
| Primary user | Who calls the tome CLI day-to-day? | Agents invoke tome at runtime; humans author workflows (via the skill). |
| Workflow format | How is a workflow definition stored? | Markdown with structured frontmatter. |
| Storage | What does the database hold? | Run state only (runs, step status, history, logs). Use **DuckDB instead of SQLite**. |
| Runtime | Where does the orchestration engine run? | A long-running daemon; CLI commands talk to it. |
| Harnesses | Which agent harnesses in v1? | A generic command adapter (any CLI agent via a configurable command template). |
| herder | What is "herder"? | Third-party tool: herdr — https://herdr.dev/docs/ |
| Data flow | How do steps pass data to each other? | Files on disk, git worktrees, etc. |
| Completion | When does a step count as done? | Agent calls tome; process exits; timeout; human approves. |
| Primitives | Which primitives in v1? | Queue, Session, Worktree, Group. |
| Actions | Which actions besides fan-out / fan-in? | Run agent / command; notify user; send message; branch / loop. |
| Failure | Default behavior on step failure/timeout? | Users can specify failure conditions per workflow. Default is to notify the user. |
| v1 goal | Target for first usable release? | All backends (cmux, tmux, herdr) supported. |
| Reuse | How are workflows shared across projects? | Global `~/.tome/workflows` plus project `.tome/workflows`; project overrides global. |
| Notify via | How does "notify the user" reach the user? | Multiplexer-native, OS notification, webhook / push. |
| Messages | What does "on message received" listen to? | Tome queues, external sources (webhooks etc.), agent output (pane pattern-matching). |
