---
name: wayfinder
description: Clarify requirements for large efforts across sessions by keeping the goal and open clarification questions in a map and answering them one by one. Runs before writing a plan; creates no tickets or branches, a spec only with consent. Use when the way to the goal is unclear and one session is not enough.
---

# Wayfinder

Adapted from the Wayfinder skill by Matt Pocock (MIT license, github.com/mattpocock/skills). You only plan; you build nothing. The result is a map that ends in a summary, which becomes the spec under `docs/superpowers/specs/` and then a plan. Never change tickets. If the effort fits one session, use the grill skill instead.

## Prepare

Read `GLOSSARY.md`, `docs/adr/`, specs, existing maps under `docs/clarification/`, tickets (`nullradix ticket list`), notes (`nullradix note list --md`) and questions (`nullradix question list`). Decided questions are not reopened. Board commands are read-only here. Continue an existing map instead of making a second one. If the project is not set up, ask the user to run `nullradix init`.

## Map

State the goal in one or two sentences (how do we know it is done?), asking in rounds with recommendations like the grill skill. Keep one file `docs/clarification/<short-name>.md` with sections:

- **Goal**, **Frame** (constraints, hints, sources)
- **Decisions so far**: one line each (question, answer)
- **Clarification questions**: `K1`, `K2`, ... with kind (conversation, prototype, research, groundwork), state (open, in progress, settled) and "waits for"
- **Not yet fixed**: unclear things that are not yet questions
- **Out of scope**

`K` numbers belong to the map; they are not tickets.

A thing becomes a question once you can state it precisely, not once you can answer it. If the effort already looks clear, say so and suggest the grill skill. The frontier is every open question that waits for no other.

## Work the map

One question per session, except research:

1. Load the map, pick a frontier question, name it, mark it in progress.
2. Answer by kind: conversation (rounds with recommendations); research (read code, files, docs; a subagent is allowed; read tickets only; put the result in the map); prototype (throwaway code in the working folder only, never in the main branch; summarize what it answered); groundwork (what the user must do; note it).
3. Record: question settled, one line under decisions, new questions or cleared fog added.
4. Capture terms and decisions at once: vague or clashing words get a clear term in `GLOSSARY.md`; an ADR under `docs/adr/NNNN-short-title.md` only if hard to reverse, surprising and a real trade-off.

## Finish

The map is done when the frontier and "Not yet fixed" are empty and the user confirms.

1. Print a compact summary: goal, decisions with core statement, out of scope, terms, open points.
2. Several separate features become separate runs, each with its own spec and plan.
3. The summary becomes the spec `docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md` (committed); write it only with consent and do not start planning yourself.
4. Then the plan, then "create tickets for the tasks of the plan" (tasks skill), then the implement skill or the dispatcher.

## Rules

Create no tickets, specs (except at hand-over with consent), branches or worktrees; push nothing. Only the map, confirmed glossary entries and ADRs are written. Do not call other skills.
