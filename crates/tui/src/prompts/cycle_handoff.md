# Novel Studio Cycle Handoff Briefing

You are about to cross a context cycle boundary. The current transcript will be
archived to disk and the next turn will start with a fresh context: the system
prompt, structured runtime state, the user's pending message, and a compact
`<carry_forward>` briefing that you write now.

Your job in this single message: produce a `<carry_forward>` block of at most
3,000 tokens that lets the next Novel Studio turn continue the book work without
re-reading the whole conversation.

## What to Preserve

Write concrete prose, not a chronological transcript. Cover only load-bearing
state:

- Book identity: title, genre, project root, current volume/chapter, and the
  user's active creative objective.
- Current chapter state: brief/draft/final/audit status, exact target chapter,
  requested revision scope, and files that must not be touched.
- Canon constraints: facts, world rules, style rules, information boundaries,
  character states, relationship changes, injuries/resources, and decisions
  already made.
- Open promises: unresolved foreshadowing, reader promises, secrets, planned
  payoffs, and any risk of premature reveal or dropped payoff.
- Memory state: graph freshness, candidate memory updates, ledgers/summaries
  already written, and what still needs `/remember`, `memory build`, or
  verification.
- Work state: checklist item in progress, sub-agent/RLM sessions, commands run,
  verification passed/failed/not run, and changed book assets.
- Next action: one concrete writing, audit, revision, memory, export, or
  verification step the next turn should take first.

## What Not to Preserve

- Tool output bytes or long file contents. They are archived or can be read
  again from the workspace.
- Generic implementation history. The next turn needs the current book state,
  not the order of every tool call.
- External material as canon. If sources under `materials/`, MCP resources,
  web pages, or imported documents matter, record their source and note that
  they remain reference-only until promoted into bible/cards/memory.
- Pleasantries, filler, or broad advice.

## Format

Open with `<carry_forward>` on its own line. Close with `</carry_forward>` on
its own line. No prose outside the tags. No nested tags. No code fences around
the block itself.

Inside the block, use this compact shape when applicable:

```text
Book:
Current chapter:
Canon to preserve:
Open promises / memory:
Work state:
Verification:
Next action:
```

Now write your `<carry_forward>` for this Novel Studio session.
