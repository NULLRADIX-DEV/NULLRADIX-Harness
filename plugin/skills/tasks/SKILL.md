---
name: tasks
description: Cut the tasks out of a spec and plan under docs/superpowers and create one ticket per task, each self-contained with title, description, acceptance criteria, files, interfaces and test-first steps with code, command and expected result. Use when a plan should become tasks and tickets.
---

# Tasks to tickets

Cut the tasks of a feature and create board tickets: task title = ticket title, `Description:` and `Acceptance:` = description and acceptance, steps = the ticket's steps, full task text = its plan. Change tickets only through `nullradix`.

The implementer sees only their ticket, so each ticket must stand alone: what to build, how to tell it is done, which files, which code, which command, which result. Nothing abstract.

## 1. Prepare

- Read the plan under `docs/superpowers/plans/` (newest unless told otherwise) and the spec it cites. Read `GLOSSARY.md` and `docs/adr/` if present. Follow extra wishes of the user.
- Decide first which files are created or changed and what each is responsible for; one responsibility per file, keep project patterns. The tasks follow from that.

## 2. Cut

- A task is the smallest unit with its own test cycle; setup, config and docs go into the task that needs them. Split only where a reviewer could accept one and reject the next.
- Each task ends with an independently checkable result.
- Phases: setup, foundations (block everything), one phase per user story in spec order, polish. Create and chain tickets in that order. Tasks that can run in parallel (other files, no open dependency) get no dependency on each other.

## 3. Handbook

The handbook of every ticket holds what applies to each task, from plan, spec and project rules (invent nothing): goal in one sentence, approach in two or three, technology (language, libraries, test tool), and a **Rules** list with one line per rule, values verbatim (versions, language of texts, "one class per file").

## 4. Task format

The first line is the title; details are indented two spaces, continuation lines four.

````markdown
Show ticket history

- Description: The ticket page shows its history below the description, newest first. Without entries it says "No history yet". Source: FR-004.
- Acceptance: Three entries appear in reverse order; an empty history shows the hint
- Files: web/src/TicketHistory.tsx (new), web/src/TicketHistory.test.tsx (new)
- Uses: GET /api/tickets/:id/history; type TicketEntry
- Provides: <TicketHistory entries={TicketEntry[]}>
- [ ] Write the test in web/src/TicketHistory.test.tsx
  ```tsx
  it('shows the hint for an empty history', () => {
    render(<TicketHistory entries={[]} />);
    expect(screen.getByText('No history yet')).toBeTruthy();
  });
  ```
- [ ] Run the test, it must fail
      Command: `npx vitest run src/TicketHistory.test.tsx` in web
      Expected: FAIL, `TicketHistory` is not defined
- [ ] Implement minimally
- [ ] Run the test, it must pass
- [ ] Commit
      Command: `git commit -m "feat: show ticket history"`
````

- Title: a short imperative sentence, about 60 characters, naming the result, no paths or identifiers.
- `Description:` required, one to four sentences: what it does, for whom, which behavior shows, where it comes from. Do not repeat the title.
- `Acceptance:` required, checkable statements with concrete values, ending with the spec reference.
- `Files:` exact paths, "(new)" for new files. `Uses:` and `Provides:` with exact names and types; omit when empty.
- Steps are `- [ ] ...`, two to five minutes each, unique within the task. The line names the action; lines below give how.
- Code steps: write test (real test code with inputs and expected values), run (exact command and expected result such as "FAIL, `Greeter` is not defined"), implement (real code), run (must pass), commit (message). Tasks without code get steps with exact text or setting, a check command and the expected result.
- A Mermaid diagram may go into a step's guidance, never into description or acceptance.

## 5. No placeholders

Never write: "TBD", "add later", "add error handling" or "handle edge cases" without saying which and with what result, "write tests" without test code, steps that say what but not how, "as in the previous task", names or types no task defines, a description that repeats the title, acceptance without concrete values.

## 6. Self-check

Before creating tickets check once yourself: every requirement and story is covered; each task is self-contained (could you build and verify it from the ticket alone?); no placeholders; names and types agree between code, `Uses:` and `Provides:`; titles are short and unambiguous.

## 7. Create

- Commit spec and plan first: worktrees and agents see only the last commit (`ticket start` warns otherwise).
- Check `nullradix ticket list` for duplicates; if a task has a ticket, name it instead of creating one.
- Per task in phase order:

```bash
nullradix ticket create "<title>" --description "<description>" --acceptance "<acceptance>" \
  --step "<step 1>" --step "<step 2>" --depends-on <previous key>
nullradix plan write <KEY> --plan-file task.md --handbook-file handbook.md --diagram-file diagram.mmd
```

`--depends-on` names the previous task's ticket (none for the first and for parallel tasks). The plan file is the full task text with code and ends with `## Effort` (`small`, `medium` or `large`, then a reason); the diagram is a small Mermaid diagram without a code fence. A named target branch goes on every `ticket create` as `--target-branch`.

- Tickets land on the board bound to the repository; if none is, show `nullradix board list` and `nullradix board repo <board> <path>`.
- Tell the user the tickets created, tasks per story, parallel options and a suggested first slice. Suggest the implement skill. For the dispatcher add the label `auto` (`ticket set KEY --label ...` replaces all labels, repeat the existing ones).

## Rules

Create tickets only with `nullradix ticket create` and `plan write`. Create no worktrees or branches. Do not tick steps; `step check` and `ticket done` do.
