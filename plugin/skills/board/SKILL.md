---
name: board
description: Use the NULLRADIX board - find, create, claim, plan, comment on, finish and block tickets, ask questions and keep notes. Use before any multi-step task, for keys like ARC-7, when a plan is approved, when asked what is open, when you notice a finding about something else, when a decision from the user is missing, and when a ticket should be prepared for the dispatcher. Not for Azure DevOps, Jira or GitHub.
---

# Board

The board is the task tracker. Change tickets only through `nullradix`, never by hand in the database. Without `--board`, commands use the board bound to the current repository, else the active board (`nullradix board list`, `nullradix board use <id>`). If a command says the daemon is not running, tell the user and stop.

## Rules

1. Before a multi-step task, look for a ticket (`nullradix ticket list --query "word"`); create one only if none exists.
2. Claim before you work, comment on progress, finish with acceptance.
3. An approved plan goes onto the ticket before the first code change.
4. No `done` without acceptance: the ticket says how to tell it is finished.
5. A missing decision from the user is a question (`nullradix question add ... --ticket KEY`), not a blocked reason.
6. Order belongs in dependencies (`--depends-on`), not in the description.
7. A finding about something else is a note, not a comment and not a ticket on suspicion.
8. Pass `--author claude` (`--agent claude` for claim) so the history shows who acted.
9. Never use `--force`, delete, switch or create boards or projects without being told to.

## Columns

`backlog`, `in-planning`, `planned`, `in-progress`, `done`, plus `blocked` and `not-needed`.

- `not-needed` and `in-planning` belong to the user; `move` and `claim` refuse them. If a ticket looks superfluous, comment.
- Tickets with an open dependency or question wait; claim and move to in-progress or done fail.
- Order within a column follows priority, then number (`ticket set KEY --priority high`).

## Read first

```bash
nullradix ticket list [--status planned] [--needs-planning] [--query word] [--board all]
nullradix ticket show ARC-7 [--all-comments] [--history] [--json]
nullradix inbox --md [--kind merge,accept]
```

`inbox` shows what waits for the user: failed merges, questions, acceptances, blocked tickets, notes. Deciding and accepting stay with the user.

## Create

```bash
nullradix ticket create "Title" --description "Context" --acceptance "How to tell it is done" \
  --priority high --label backend --step "First step" --depends-on ARC-3
```

`--label` and `--step` repeat. `--planned` skips planning. A duplicate open title is refused; look first. `--target-branch` names the branch the ticket merges into.

## Plan

```bash
nullradix plan next --claim claude          # next ticket that needs planning, claimed 15 minutes
nullradix plan write ARC-7 --plan-file plan.md --handbook-file handbook.md --diagram-file d.mmd
```

`plan write` sets `planned` and clears the planning need. The plan ends with `## Effort`: first word `small`, `medium` or `large`, then a short reason; the dispatcher picks model and effort from it. The diagram is Mermaid without a code fence. To change one field only, use `ticket set KEY --plan-file plan.md`.

For plans from a document, see the tasks skill.

## Work

```bash
nullradix ticket claim ARC-7 --agent claude
nullradix ticket start ARC-7                 # own branch and worktree in a repository with a board
nullradix step check ARC-7 2                 # --undo to uncheck
nullradix ticket comment ARC-7 "Step 2 of 4 done, tests run." --author claude
nullradix ticket done ARC-7 --acceptance "Test covers the case" --author claude
```

- An assigned ticket makes `claim` fail. If you claimed it earlier in this session, go on; otherwise take the next free ticket or ask.
- `done` cleans up a finished worktree; nothing is merged. Only the dispatcher or a person merges (`ticket merge KEY` retries).
- Steps: `step add|edit|move|remove KEY ...`. Numbers shift after add, move and remove; read the ticket again. `--expect` and `--version` guard against concurrent edits; on a conflict read again and retry.

## Where does finished work go

| Outcome                                 | When                                                          |
| --------------------------------------- | ------------------------------------------------------------- |
| `done`                                  | finished and verified by you; evidence in a comment           |
| `blocked` with `Human Interaction: ...` | finished, but you cannot verify it yourself                   |
| question with `--ticket`                | not finished, a decision is missing                           |
| `blocked` with a reason                 | not finished, something else is missing (access, environment) |

```bash
nullradix ticket set ARC-7 --status blocked --blocked-reason "Human Interaction: look at the page" --author claude
```

Only the user accepts a `Human Interaction` (`ticket accept KEY`).

## Questions and notes

Questions: see the question skill for the writing rules and research. Read the comments and questions of a ticket before you start (`nullradix question list --ticket ARC-7 --state all`); a decision beats plan and handbook.

```bash
nullradix note list --md
nullradix note add "One sentence: what is broken" --details "Where, how to see it, cause, proposal" --from ARC-7 --label tests --author claude
nullradix note seen 12 --from ARC-9
```

- List first. If the finding exists, `note seen` counts it; `note add` refuses likely duplicates (`--force` only if it really is different).
- Write for someone who does not know your ticket: no ticket key in the title, no "unrelated to ...". The origin goes in `--from`.
- Reuse existing labels. Promote notes (`note promote N`) or drop them (`note drop N --reason "..."`) only when asked.

## Prepare a ticket for the dispatcher

The dispatcher takes a ticket that is `planned`, unassigned, has plan, handbook and acceptance, no open dependency or question, the label `auto` and not `manual`, on a board with a repository folder.

1. Read the ticket and `nullradix dispatcher status` (it names what is missing).
2. Fill the gaps from the code: plan (see Plan), acceptance (`ticket set KEY --acceptance "..."`), steps. A missing user decision becomes a question; report open dependencies and a missing repository, do not work around them.
3. Set labels: `ticket set KEY --label auto` replaces all labels, so repeat the existing ones. Add `model:` or `effort:` labels only if the user asks.
4. Comment the effort estimate and what you added; run `dispatcher status` again and report. The dispatcher starts the ticket itself.

## Started by the dispatcher or the plan button

- Dispatcher agents are named `worker-<KEY>`; the ticket is already claimed and the worktree exists. Do not claim, start or touch other tickets; read `ticket show KEY` first; finish with exactly one outcome from the table, always with `--author`. Do not push, merge or switch branches; no links to other folders.
- Planning agents (`planner-<KEY>`): do not claim, move or label; read the ticket and its questions; end with `plan import` or a question.
- Comments starting with `[dispatch]` belong to the dispatcher; never delete or imitate them.
- In autopilot there is no `Human Interaction`: check what you can, then finish with `done`.
