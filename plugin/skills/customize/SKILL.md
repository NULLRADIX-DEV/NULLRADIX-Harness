---
name: customize
description: Manage NULLRADIX customizations - replace a shipped skill or output style or add your own, check, disable and reset them. Use when the user wants to change how NULLRADIX works, replace a NULLRADIX skill or output style, or add one.
---

# Customize NULLRADIX

Customizations are skills and output styles (`<point>` is `skill` or `output-style`) the user replaced or added. They are stored by the daemon and apply to all projects after `nullradix install` and a new session (or `/reload-plugins`).

## Commands

| Task                                | Command                                                |
| ----------------------------------- | ------------------------------------------------------ |
| See all with state, problems, hints | `nullradix customize overview`                         |
| Print the text in charge            | `nullradix customize show <point> <name>`              |
| Start from the shipped file         | `nullradix customize copy <point> <name>`              |
| Create or replace from a file       | `nullradix customize set <point> <name> --file <file>` |
| Apply again                         | `nullradix customize enable <point> <name>`            |
| Keep without applying               | `nullradix customize disable <point> <name>`           |
| Back to the shipped file            | `nullradix customize reset <point> <name>`             |

## How

- Replacing starts with `copy`: it starts at the current shipped text and remembers the base, so later changes of the shipped file are reported. Edit the copy through a file and `set`. `copy --rebase` only after taking over a changed shipped file.
- A new skill or output style is written to a file with a head (`name: <name>`, `description`) and stored with `set`. A name equal to a shipped one replaces it; a new name adds one.
- Names: lowercase letters, digits and single hyphens, at most 64 characters.
- After every change run `overview`: invalid customizations fall back to the shipped file; the output names the file and the reason. Then offer `nullradix install`.
- If `overview` says the shipped file changed since the copy, show the user the difference, merge what they want into the customization, then `copy --rebase`.
- Confirm before `reset` and say what is dropped; there is no backup.
