# tome

Repeatable agentic workflows. Write a workflow once as a Markdown file, in
plain English, and say what starts it: a file change, a cron schedule, a
message, or another workflow finishing. The tome daemon starts an orchestrator
agent each time, which spawns worker agents in your terminal multiplexer (cmux
or tmux) where you can watch and steer them.

**Docs: <https://banyango.github.io/tome>**

Run `tome --help` for the commands and flags.

## Building from source

```sh
cargo build --release
```

The binary is `target/release/tome`.

## License

[MIT](LICENSE)
