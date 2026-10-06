# Custom Step Statuses

## Description

Workflow authors can assign a free-form custom status to a step. The run's custom status follows its active step and is stored in a database column separate from the built-in run status. Status labels match exactly, including case, and multiple steps can share a label.

When the active step has no custom status, the run has no custom status and is unassigned for a future swimlane viewer. When a run completes or fails, it retains the last custom status. If parallel work is active, the parent or group step supplies the run's custom status.

This feature establishes status declarations and run-level status tracking. User-defined swimlane layouts and the TUI viewer are future features. A later viewer can place a run under a matching lane, including in layouts shared across workflows.

## Use Cases

1. As a workflow author, I want to assign custom status labels to steps, so that I can define meaningful stages for runs in my workflow.
2. As a workflow author, I want multiple steps to share a status label, so that those steps correspond to the same swimlane lane.
3. As a user tracking a run, I want its custom status to follow its active step independently of its built-in run status, so that a future swimlane viewer can show where the run is in my workflow.

## Constraints

- Custom status is separate from built-in run status and stored in its own database column.
- Labels are free-form and match exactly, including case.
- When the active step has no custom status, the custom run status is unset; a future viewer may represent this as an Unassigned lane.
- Parallel work uses the parent or group step's custom status.
- Completed and failed runs retain their last custom status.
- Swimlane layout definitions, CLI inspection, and the TUI viewer are out of scope.

## Edge Cases

- Active step has no custom status → clear the run's current custom status; the future viewer can show it as unassigned.
- Multiple active parallel steps have different statuses → use the parent or group step's status.
- Run completes or fails → preserve its last custom status while built-in run status records the terminal state.
- Multiple steps share a label → treat them as the same status.
- Labels differ only by case → treat them as different statuses.

## Related Entities

- [[workflow]]
- [[step]]
- [[run]]
