---
name: grill
description: Clarify requirements with the user in rounds of questions with recommendations, and keep the glossary and ADRs current. Runs before writing a plan; creates no tickets or worktrees. Use when a plan, idea or decision needs clarifying.
---

# Grill

Interview the user until you share one understanding. The result is a summary that becomes a spec under `docs/superpowers/specs/` and then a plan. Create no tickets, branches or worktrees, and a spec only with consent.

## Prepare

Read what exists: `GLOSSARY.md`, `docs/adr/`, specs under `docs/superpowers/specs/`, tickets (`nullradix ticket list`). If a command says the project is not set up, ask the user to run `nullradix init` once.

## Rounds

Treat decisions as a tree; the frontier is every decision whose prerequisites are settled.

- Ask the whole frontier at once, numbered, each with a recommendation phrased so that "yes" accepts it.
- Wait for answers before the next round; answers move the frontier.
- Look facts up yourself (code, files, docs). Ask the user only for decisions.
- Done when the frontier is empty and nothing was assumed silently. Summarize briefly and get it confirmed.

Format of a question:

```
Q1 - <title>: <question, with options if any>
Recommendation: <your answer>
```

## Capture while talking

- Terms: when a word clashes with the glossary or is vague, say so and propose a clear term. Write settled terms to `GLOSSARY.md` at once (term, one or two sentences, "Do not use" for synonyms). Project terms only.
- ADR: offer one under `docs/adr/NNNN-short-title.md` only if the decision is hard to reverse, surprising without context and the result of a real trade-off. A paragraph is enough: context, decision, reason.
- Test relationships with concrete examples and edge cases.

## Hand over

1. Print the summary: goal, decisions, edge cases, terms, open points.
2. Point out the next step: the summary becomes the spec `docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md` (committed). Write it only when the user agrees; do not start planning yourself.
3. Then the plan is written, and "create tickets for the tasks of the plan" uses the tasks skill; implementing follows with the implement skill or the dispatcher.

## Rules

Create only glossary entries and confirmed ADRs, plus the spec at hand-over with consent. Never change tickets.
