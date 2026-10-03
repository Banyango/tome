# Backend

## Definition

A terminal multiplexer that tome uses as its UI layer. Tome coordinates backends to create [[session]]s and show work in progress.

## Attributes

- v1 backends (all three are required for v1):
  - cmux
  - tmux
  - herdr (https://herdr.dev/docs/; written as "herder" in `intent.md`). Left out of v1 (010); specified in [015](../features/015-herdr-backend/feature.md): reached through herdr's socket API, works without a visible client, and reports agent status (`blocked` becomes a tome notification and a `tome.run.<workflow>.blocked` event)
- also delivers multiplexer-native [[notification]]s

## Relationships

- [[session]]: backends host sessions
- [[daemon]]: the daemon drives backends
- [[notification]]: backends are one notification channel

## Planned Features

- Terminal backends: one [[session]] interface over cmux, tmux and herdr _(source: .sengify/sources/feature-set-2026-09-26.md)_

## Sources

- intent.md
- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
- features/015-herdr-backend/feature.md
