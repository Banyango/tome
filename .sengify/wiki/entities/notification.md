# Notification

## Definition

A message that tells the user something needs attention: a failure, a request for approval, or a status update.

## Attributes

- channels:
  - multiplexer-native (through the [[backend]])
  - OS notification (macOS)
  - webhook / push (Slack, ntfy, etc.)
- default use: the default response when a [[step]] fails

## Relationships

- [[action]]: produced by the notify-user action
- [[step]]: human approval, one way a step completes, may start with a notification
- [[backend]]: one delivery channel

## Planned Features

- Notifications and human approval _(source: .sengify/sources/feature-set-2026-09-26.md)_

## Sources

- intent.md
- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
