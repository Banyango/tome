# tome

Agentic workflow management CLI. tome runs multi-step workflows written as
Markdown files, starting agent workers in your terminal multiplexer (cmux or
tmux) and coordinating them through worktrees, queues and triggers.

**Docs: <https://banyango.github.io/tome>**

Run `tome --help` for the commands and flags.

## Building from source

```sh
cargo build --release
```

The binary is `target/release/tome`.

## License

[MIT](LICENSE)
