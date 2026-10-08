# Custom Step Statuses — Task Plan

Source feature: [feature.md](./feature.md)

## Tasks

### 018-1. Declare custom statuses on steps

**Blocked by:** none

Workflow authors can attach a free-form status label to a step in the workflow definition. Labels are kept exactly as written (case-sensitive), several steps can share one label, and steps without a label are valid. Covers use cases 1 and 2.

### 018-2. Track run custom status in its own column

**Blocked by:** 018-1

Runs gain a custom status stored separately from the built-in run status. When a step becomes active, the run's custom status is set to that step's label, or cleared if the step has none. Covers use case 3.

### 018-3. Parallel and terminal status rules

**Blocked by:** 018-2

While parallel work is active, the run takes the parent or group step's custom status instead of the child steps' statuses. When a run completes or fails, it keeps its last custom status while the built-in status records the terminal state. Covers use case 3.

## As built

- Declared in plain English in the step's text ("Set status to InReview"). The agent carries it out with `tome step status "<status>" [--step <name>]` (018-1). The built-in agent and orchestrator prompts tell it to.
- `steps.custom_status` and `runs.custom_status` (migration 16). The run's column follows its active step and is recomputed on every step report or status change (018-2). `tome runs show` lists it as `custom` on the run and as a `CUSTOM` column on the steps.
- Steps have no hierarchy, so the parent or group step is the earliest-started step that is still running (018-3). With no step running, the run's status stays as it was, so a finished run keeps its last one. A `tome step status` call with no step running sets the run's status directly.
