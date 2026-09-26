# TOME CLI

This project is the cli for Tome. Tome is a agentic workflow management CLI. We use it to create workflows and run them. 

## Problem Statement
Right now agentic workflows are hard to orchestrate, once you have a workflow repeating it means a mess of md files and things. We are also constrained to a single harness. Terminal multiplexors are great for orchestrating a workflow and watching it happen/ responding to issues that pop up. However if I get a workflow running in one session it's difficult to copy that workflow to a new project or alter it subtly. 

## Solution
We're going to create a CLI that agents can use to orchestrate their coordination. Cmux, tmux and [herdr](https://herdr.dev/docs/) will be our UI layer and the cli will be able to orchestrate them together. 

We'll build a skill that allows users to specify workflows. The skill will outline the steps the workflow should take. The workflow should be able to run Agents, spawn new terminal multiplexor windows, run CLI commands, notify the user, etc.  

Our CLI can have primitive objects like a queue, single terminal session, etc.

We have triggers like on file created/ edit, on group of tasks complete, on message recieved

We have actions like fan out, fan in (consolidate), others.

## Technical Details
- Rust
- DuckDB for run state (runs, step status, history, logs).
- CLI.
- 
