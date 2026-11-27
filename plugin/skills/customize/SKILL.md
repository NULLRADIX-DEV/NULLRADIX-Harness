---
name: customize
description: Manage NULLRADIX customizations - store, list, show, disable and reset replacement or extra skills and output styles. Use when the user wants to keep or review a custom skill or output style.
---

# Customize NULLRADIX

Customizations are skills and output styles (`<point>` is `skill` or `output-style`) that the user stored in the daemon. They are only stored and listed for now: NULLRADIX does not yet put them into the installed plugin, and there is no shipped text to copy from. Say so if asked whether a change is active.

## Commands

| Task                                | Command                                                |
| ----------------------------------- | ------------------------------------------------------ |
| See all with state, problems, hints | `nullradix customize overview`                         |
| Print the stored text               | `nullradix customize show <point> <name>`              |
| Create or replace from a file       | `nullradix customize set <point> <name> --file <file>` |
| Apply again                         | `nullradix customize enable <point> <name>`            |
| Keep without applying               | `nullradix customize disable <point> <name>`           |
| Drop it                             | `nullradix customize reset <point> <name>`             |

## How

- Write the whole text to a file with a head (`name: <name>`, `description`) and store it with `set`.
- Names: lowercase letters, digits and single hyphens, at most 64 characters.
- After every change run `overview` and report its problems and hints.
- Confirm before `reset` and say what is dropped; there is no backup.
