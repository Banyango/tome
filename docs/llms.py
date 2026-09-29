#!/usr/bin/env python3
"""Publish the docs as plain text for agents.

Run after `mdbook build`, from the docs directory:

    python3 llms.py <site-url>

Writes into book/:
  llms.txt       an index of the pages
  llms-full.txt  every page joined into one file
  md/<page>.md   each page's Markdown, with `{{#include}}` lines resolved
"""
import re
import sys
from pathlib import Path

SITE = (sys.argv[1] if len(sys.argv) > 1 else "https://banyango.github.io/tome").rstrip("/")
SRC = Path("src")
OUT = Path("book")

INCLUDE = re.compile(r"^\{\{#include\s+(\S+?)\s*\}\}\s*$")
LINK = re.compile(r"\[([^\]]+)\]\(([^)]+\.md)\)")


def resolve(path: Path) -> str:
    lines = []
    for line in path.read_text().splitlines():
        m = INCLUDE.match(line)
        if m:
            lines.append((path.parent / m.group(1)).read_text().rstrip("\n"))
        else:
            lines.append(line)
    return "\n".join(lines) + "\n"


def pages():
    """(title, path, section) in book order, from SUMMARY.md."""
    section = None
    for line in (SRC / "SUMMARY.md").read_text().splitlines():
        if line.startswith("# ") and not line.startswith("# Summary"):
            section = line[2:].strip()
        m = LINK.search(line)
        if m:
            yield m.group(1), m.group(2), section


def main():
    entries = list(pages())
    (OUT / "md").mkdir(parents=True, exist_ok=True)
    index = [
        "# tome",
        "",
        "> tome runs agentic workflows: Markdown files that describe steps, which an orchestrator agent carries out with workers in your terminal multiplexer (cmux or tmux).",
        "",
        f"Every page below is plain Markdown. The whole site as one file: {SITE}/llms-full.txt",
    ]
    full = []
    current = object()
    for title, rel, section in entries:
        text = resolve(SRC / rel)
        dest = OUT / "md" / rel
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(text)
        if section != current:
            index += ["", f"## {section or 'Overview'}", ""]
            current = section
        index.append(f"- [{title}]({SITE}/md/{rel})")
        full.append(text)
    (OUT / "llms.txt").write_text("\n".join(index) + "\n")
    (OUT / "llms-full.txt").write_text("\n\n---\n\n".join(full))
    print(f"wrote llms.txt and llms-full.txt for {len(entries)} pages")


main()
