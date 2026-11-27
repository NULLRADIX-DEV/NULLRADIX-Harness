---
name: implement
description: Implement a board ticket test-first (by key, or the next planned one) in the worktree NULLRADIX creates, keep steps, comments and status current, and finish with review and acceptance. Use when a ticket should be implemented.
---

# Implement a ticket

Change status, steps and comments only through `nullradix`. NULLRADIX creates worktrees, not you.

## 1. Pick and understand

- With a key: only that ticket (`nullradix ticket show ARC-42`). Without: `nullradix ticket list --status planned`; name the ticket you take and stay within its spec.
- No planned ticket: say so and ask. New tickets need planning first (`nullradix plan next`). If it waits on a dependency or question, name it and suggest finishing that first. Never decide questions yourself.
- Read description, acceptance, steps, plan, handbook and comments, plus the spec and plan documents they cite. State title and goal in one sentence before you start.

## 2. Environment

- `nullradix ticket claim ARC-42 --agent claude`. If it is assigned to someone else, take the next planned ticket or ask.
- `nullradix ticket start ARC-42` creates the branch and worktree (`--join` joins an open worktree of the same target branch). If it says the ticket belongs to another board than this repository's, name both boards and ask.
- Work in the folder it names. Never create worktrees or branches yourself; do not use a worktree skill.

## 3. Build test-first

1. Split the work into small steps. Implement each part with a subagent with fresh context (`superpowers:subagent-driven-development` when installed); do not implement in the main run. Subagents never change the ticket.
2. Write a failing test and see it fail, implement, refactor (`superpowers:test-driven-development` when installed). Check off each step right after it is done: `nullradix step check ARC-42 1`.
3. Follow the project rules in `CLAUDE.md`: small functions, brief comments.
4. Run type check and affected tests regularly and the full suite once at the end. Never claim "done" without a command result.
5. Have a subagent review the diff before the ticket goes to done.
6. Commit in the worktree with meaningful messages.

## 4. Keep the ticket current

- Comment on every change: `nullradix ticket comment ARC-42 "Step 2 of 4 done, tests green" --author claude`.
- Comments of the user reach you with the next prompt; consider them and answer by comment.
- A missing decision becomes a question (question skill); the ticket waits for it.

## 5. Finish

- Tests and type check green and review done: comment result, changed areas, checks and open points, then `nullradix ticket done ARC-42 --acceptance "How to tell it is done" --author claude`. This sets done and cleans up the worktree; nothing is merged.
- Something open (red tests, review remarks, questions): the ticket stays in progress and the comment says what is missing. If you could not verify it yourself, block it with `Human Interaction: ...` (board skill).
- Another planned ticket of the same spec: continue with it without asking. Never take tickets of other specs unasked.
- Stop when no ticket of the spec is ready, a review has open points, tests stay red or a decision is needed; then name branch, worktree and the next step.

## Rules

Do not merge or push unless asked. Do not edit tickets by hand.
