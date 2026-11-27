---
name: customize
description: Manage NULLRADIX customizations - copy a shipped skill or output style, edit, disable and reset it. Use when the user wants to change or review a skill or output style.
---

# Customize NULLRADIX

Customizations are skills and output styles (`<point>` is `skill` or `output-style`) that the user stored in the daemon. They start from the shipped text and replace it, or add a new name. `nullradix install` puts every active customization on top of the shipped plugin files and updates the accounts. A change is live only after `nullradix install`.

## Commands

| Task                                | Command                                                |
| ----------------------------------- | ------------------------------------------------------ |
| See all with state, problems, hints | `nullradix customize overview`                         |
| Print the text in charge            | `nullradix customize show <point> <name>`              |
| Start from the shipped text         | `nullradix customize copy <point> <name>`              |
| Take a newer shipped text as base   | `nullradix customize copy <point> <name> --rebase`     |
| Create or replace from a file       | `nullradix customize set <point> <name> --file <file>` |
| Apply again                         | `nullradix customize enable <point> <name>`            |
| Keep without applying               | `nullradix customize disable <point> <name>`           |
| Drop it                             | `nullradix customize reset <point> <name>`             |
| Put the changes into the plugin     | `nullradix install`                                    |

## How

- Start with `show` of the shipped name, edit the text in a file with its head (`name: <name>`, `description`), store it with `set`.
- Names: lowercase letters, digits and single hyphens, at most 64 characters.
- After every change run `overview` and report its problems and hints; a hint `base-outdated` means the shipped text changed, review it and run `copy --rebase`.
- Run `nullradix install` to apply; if the daemon is not running it stages the shipped files only and says so.
- Confirm before `reset` and say what is dropped; there is no backup.
