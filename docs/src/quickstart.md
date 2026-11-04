# Quickstart

In this walkthrough you set up a workflow that runs on its own: whenever you add a feature file to your project, agents investigate the codebase and break the feature into tasks. You need tmux, Claude Code, and tome [installed](install.md), and a git repository with some code in it.

## 1. Add the workflow

Install the tome [skill](agents/skill.md) if you haven't already.

```markdown
/tome Generate a test workflow named my-workflow
```

Tome will guide you through important questions for your workflow

## 2. Run your workflow

```markdown
> Use tome to run my-workflow
```

## 3. Enjoy

It's that easy.

## Next

- [Create workflows with an agent](guides/create-with-an-agent.md): describe what you want and let your agent write the workflow.
- [Recipes](recipes.md): implementing planned features, fixing failing tests, reviewing branches, and more.
- [Triggers and the message bus](guides/triggers.md): schedules, messages, and chaining workflows together.
- [Write a workflow](guides/write-a-workflow.md): every frontmatter option.
