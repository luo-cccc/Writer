# Writer

> Terminal-native long-form fiction studio for DeepSeek V4. It turns a novel into local project assets: story bible, world rules, character cards, chapter briefs, drafts, audits, revisions, continuity memory, and exports.

[简体中文 README](README.zh-CN.md)

## What Is It?

Writer is a local-first novel creation tool. It keeps the book on disk instead of burying it in chat history, so a long project can survive hundreds of chapters without losing continuity.

It is built around DeepSeek V4 (`deepseek-v4-pro` / `deepseek-v4-flash`), including 1M-token context windows and thinking-mode responses. The existing terminal runtime remains available, but the default product identity and workflow now target fiction production.

## Core Workflow

```bash
deepseek init --title 我的长篇 --genre 都市重生 --premise 主角带着失败记忆回到十年前
deepseek plan --chapters 30
deepseek brief 1
deepseek memory build
deepseek memory context 1
deepseek write 1 --words 3500
deepseek audit 1
deepseek revise 1
deepseek remember 1
deepseek export --format markdown
```

## Project Layout

```text
book.toml
bible/
  premise.md
  world.md
  reader_promise.md
  style.md
craft/
  human_texture.md
  anti_ai_patterns.md
cards/
  characters/
  world/
  locations/
outline/
  master_plan.md
  chapter_index.md
chapters/
  001/
    brief.md
    craft_plan.md
    draft.md
    audit.md
    final.md
memory/
  graph.json
  facts.jsonl
  events.jsonl
  foreshadowing.jsonl
  summaries/
  characters/
exports/
```

## Key Features

- Book initialization with reusable story-bible templates.
- Master planning for worldbuilding, cast, volume arcs, and chapter beats.
- Chapter briefs before drafting, so each chapter has a purpose, conflict, and hook.
- Long-form memory graph: book assets, chapters, characters, world rules, locations, promises, state changes, objects, and summaries become nodes with evidence-backed story edges.
- Token-efficient memory context packets for the next chapter, so drafting reads the relevant relationship neighborhood instead of flattening the whole book.
- Optional craft notes with `deepseek empower`; they are not a scoring template and do not constrain the draft.
- Chapter drafting against current bible, outline, recent chapters, and summaries.
- Memory diagnostics for character drift, timeline issues, setting conflicts, missing graph updates, and affected future material.
- Revision into accepted `final.md` chapters.
- Memory extraction after chapters creates reviewable candidates before facts, events, foreshadowing, and character state enter the ledgers.
- Markdown/TXT export of the manuscript.

## Install

`deepseek` is distributed as Rust binaries: the dispatcher command (`deepseek`) and the companion runtime (`deepseek-tui`). Existing install paths still work.

```bash
npm install -g deepseek-tui

cargo install deepseek-tui-cli --locked
cargo install deepseek-tui --locked
```

For local source installs after editing this repository:

```bash
cargo install --path crates/cli --locked --force
cargo install --path crates/tui --locked --force
```

## Authentication

On first launch you will be prompted for a DeepSeek API key. You can also set it ahead of time:

```bash
deepseek auth set --provider deepseek
deepseek doctor
```

## Documentation

- [Novel Studio workflow](docs/NOVEL_STUDIO.md)
- [Install guide](docs/INSTALL.md)
- [Configuration](docs/CONFIGURATION.md)
- [MCP integration](docs/MCP.md)
