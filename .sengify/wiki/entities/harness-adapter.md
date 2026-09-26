# Harness Adapter

## Definition

A generic, configurable command template for launching any CLI agent. It keeps tome from being tied to a single harness.

## Attributes

- command template: configurable per agent

## Relationships

- [[action]]: the run-agent action uses the adapter
- [[session]]: the adapter launches agents inside sessions

## Planned Features

- Harness adapter: the command template format is still open _(source: .sengify/sources/feature-set-2026-09-26.md)_

## Sources

- intent.md
- .sengify/sources/interview-2026-09-26.md
- .sengify/sources/feature-set-2026-09-26.md
