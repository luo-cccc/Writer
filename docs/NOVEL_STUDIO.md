# Writer

Writer is the local-first workflow for planning, drafting,
diagnosing, revising, remembering, and exporting long-form fiction. The product
goal is to treat a novel as a durable local project, not as a single chat turn.

## Project Layout

Run:

```bash
deepseek init --title 我的长篇 --genre 都市重生 --premise 主角带着失败记忆回到十年前
```

The command creates:

```text
book.toml
bible/
  premise.md
  world.md
  style.md
craft/
  human_texture.md
  anti_ai_patterns.md
  examples/
cards/
  characters/
  resources/
  world/
  locations/
materials/
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
  graph.schema.json
  SCHEMA.md
  facts.jsonl
  events.jsonl
  foreshadowing.jsonl
  behavior.jsonl
  candidates/
  summaries/
  characters/
  reports/
eval/
  failures/
  fixtures/
  rubrics/
experiments/
  reports/
exports/
```

## Core Commands

```bash
deepseek status
deepseek plan --chapters 30
deepseek brief 1
deepseek memory build
deepseek memory context 1
deepseek write 1 --words 3500
deepseek audit 1
deepseek revise 1
deepseek remember 1
deepseek memory candidates --chapter 1
deepseek memory apply --chapter 1
deepseek memory regression --window 10
deepseek memory resource-ledger
deepseek export --format markdown
```

The model writes prose and editorial artifacts. Rust owns the filesystem layout
and writes only known project files.

## Design Principles

- Local-first book assets, readable as Markdown, TOML, YAML, and JSONL.
- The project files are the source of truth; chat history is temporary.
- Long-form consistency matters more than one-shot text generation.
- Chapter work is staged: brief, memory context, draft, memory diagnostic,
  final, memory candidates, confirmed memory update.
- Long-form memory graph management is the core context system. Book assets,
  chapters, characters, world rules, locations, promises, state changes, objects,
  summaries, and chapter order become nodes with evidence-backed story edges.
- `deepseek memory context N` returns a small relationship neighborhood for chapter
  N. Drafting should use this as relevant memory, not flatten the whole book.
- `deepseek empower N` remains optional. It is a writing note, not a scoring rubric
  or mandatory creative pipeline.
- Continuity is tracked through facts, events, foreshadowing, summaries, and
  character/world cards, then connected through `memory/graph.json`.
- `deepseek remember N` stages reviewable updates in `memory/candidates/`.
  `deepseek memory apply --chapter N` confirms them into the continuity ledgers
  and rebuilds the graph.
- The default system prompt is a fiction workflow prompt focused on book assets,
  continuity, character agency, and prose texture.
- Competitor-inspired workflow points retained here: setting workshop,
  graph-shaped memory, incremental context selection, chapter-level planning,
  character state tracking, foreshadowing ledgers, memory diagnostics, and
  export. No competitor code or prompts are copied.

## Current Product State

The current implementation provides the CLI-backed Novel Studio workflow as the
main path:

- `init` creates a readable novel workspace with bible, craft, cards, outline,
  chapters, memory, evaluation, experiment, and export directories.
- `brief` creates chapter briefs with scene function, continuity pressure, and
  optional A/B/C `SceneGear`.
- `empower` creates optional craft notes. It is advisory, not a mandatory
  scoring rubric.
- `write` builds a `ContextQualityReport` before drafting and injects context
  from the book bible, chapter brief, craft plan, memory graph, recent summaries,
  behavior ledger, resource cards, and authorized reference examples.
- `audit` starts from a deterministic `ChapterQualityReport`, then asks the LLM
  for continuity and craft diagnosis.
- `revise` is targeted: it extracts the top actionable problems and edits those
  targets instead of rewriting the whole chapter by default.
- `remember` writes a summary and stages reviewable memory candidates. If the
  LLM outputs no candidates while durable changes are visible, a deterministic
  fallback extracts conservative candidates from the summary/chapter text.
- `memory apply` is the only path that writes durable ledgers.
- `memory regression N` reports each N-chapter window with workflow gate,
  candidate pressure, summary density, anchor carry, active promises, and
  regression warnings for 10/20/50-chapter review.

The legacy `deepseek novel ...` namespace remains available for compatibility,
but it is no longer the main product path.

## Quality Signals

Quality signals are represented as typed records with `code`, `severity`,
`category`, and `message`. Supported severities are:

- `blocker`
- `major`
- `minor`
- `signal_only`

`ContextQualityReport` checks whether the context packet is ready for drafting:

- memory graph state
- chapter brief and craft plan presence
- character-card coverage
- recent summaries and nearby chapters
- open/progress promises
- xuanhuan/xianxia-specific rule, faction, resource, artifact, and spell anchors

`ChapterQualityReport` checks the produced chapter:

- length, dialogue, causality, promise progress, and anchor carry
- `StyleDisciplineReport`: zero-tolerance terms, budget terms, paragraph rhythm,
  AI summary/elevation sentences, and static AI-style openings
- `SceneGear` fit: A/B/C scene mode against paragraph and atmosphere density
- weak viewpoint-boundary leakage
- xuanhuan/xianxia resource anchors, resource obligations, combat observation
  chain, dialogue voice, worldbuilding-in-action, and concrete emotion texture

The static AI-opening guard specifically catches title-after-first-paragraph
openings that start with weather, ruins, smoke, night, wind, or other empty
camera shots before a viewpoint character takes an action. Environment is still
allowed when attached to a character action.

## Memory System

Durable memory is staged before it is applied:

- `memory/summaries/NNN.md` stores chapter summaries.
- `memory/candidates/NNN.json` stores reviewable updates.
- `memory/facts.jsonl`, `memory/events.jsonl`, `memory/foreshadowing.jsonl`, and
  `memory/behavior.jsonl` are written only after review/apply.
- `memory/graph.json` is rebuilt from book assets, ledgers, candidates, summaries,
  cards, chapters, and imported analysis reports.

The behavior ledger records compact character continuity:

```text
character | situation | choice | result | evidence
```

Recent behavior records are injected before drafting so characters continue from
prior choices instead of abstract personality labels.

Resource support currently includes:

- resource cards under `cards/resources/`
- resource/economy quality signals
- resource obligation checks
- `memory resource-ledger`
- applied resource entries in `memory/resources.jsonl` with source, owner,
  quantity/state, obligation, transfer/loss, evidence, and affected nodes
- graph-based impact hints for objects/resources

This is not yet a complete double-entry resource-accounting system.

## Evaluation And Experiments

The evaluation layer exists to collect deterministic fixtures and experiment
artifacts. It does not prove real reader quality or million-word capability by
itself.

- `deepseek eval collect-failure ...` archives original failure samples.
- `deepseek eval collect-non-trigger ...` archives negative controls.
- `deepseek eval seed` writes built-in positive/negative fixture pairs for the
  default deterministic signals.
- `deepseek eval coverage` reports fixture coverage by signal.
- `deepseek experiment plan ...` creates long-run experiment plans.
- `deepseek experiment snapshot ...` captures context reports, quality reports,
  audits, memory candidates, and run metadata.

Every initialized book now includes
`experiments/baselines/long_form_acceptance.md`. Treat that file as the minimum
evidence contract for capability claims.

Recommended staged validation:

```bash
deepseek experiment plan --name baseline-10 --chapters 10 --workflow memory --model deepseek-v4-flash
# run the chapter chain recorded in experiments/configs/<run_id>.json
deepseek experiment snapshot --start 1 --end 10 --run-id <run_id>

deepseek experiment plan --name baseline-30 --chapters 30 --workflow targeted_revise --model deepseek-v4-flash
deepseek experiment snapshot --start 1 --end 30 --run-id <run_id>

deepseek experiment plan --name baseline-50 --chapters 50 --workflow targeted_revise_archive --model deepseek-v4-flash
deepseek experiment snapshot --start 1 --end 50 --run-id <run_id>
```

The 10-chapter run is a smoke plus failure-triage pass. The 30-chapter run is
the first meaningful continuity and cost profile. The 50-chapter run is the
first late-context noise and memory-pressure profile. Do not make a chapter-count
or million-word capability claim without the config, snapshots, regression
reports, summaries, candidate decisions, and gate status required by the
baseline file.

## Verification Status

Recent local verification passed:

```bash
cargo fmt --all --check
cargo check -p deepseek-tui
cargo test -p deepseek-tui novel
cargo test -p deepseek-tui commands::memory
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Recent real API smoke tests also passed against the official DeepSeek endpoint:

- API connectivity through `doctor`
- model listing for `deepseek-v4-flash` and `deepseek-v4-pro`
- direct `/v1/chat/completions` generation
- Novel Studio chain: `init -> brief -> empower -> write -> audit -> remember
  -> memory candidates -> status`
- follow-up `remember` test confirmed deterministic fallback candidates are
  generated when the LLM returns no candidate updates for a short chapter

Do not treat these smoke tests as proof of million-word stability. They verify
that the command chain, API integration, and deterministic gates run.

## Known Boundaries

- The repository directory currently has no `.git` metadata, so local change
  review cannot rely on `git diff` unless the project is restored into a Git
  checkout.
- Resource impact now has a reviewable ledger surface, but source/owner/quantity
  extraction from prose remains heuristic and it is not a full double-entry
  balance system with exact downstream propagation.
- Quality gates catch defined failure modes. They do not replace real manuscript
  review.
- The dedicated full TUI workspace remains the next UI layer: project tree,
  chapter editor, cards panel, audit panel, memory panel, and export/status views.

## Next Work

1. Add CLI smoke coverage for `deepseek eval seed` and the experiment scaffold.
2. Build the dedicated TUI Novel Studio workspace.
3. Only after the above, run long-form real validation and report capability
   using measured breakdown rates, not optimistic word-count claims.
