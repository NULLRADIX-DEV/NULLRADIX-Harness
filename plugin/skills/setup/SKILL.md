---
name: setup
description: Install the NULLRADIX plugin into the Claude folders, set up the current project and find its board. Use when `nullradix` is not found, after the first installation or after an update, or when a project should use NULLRADIX.
---

# Set up NULLRADIX

Run the steps yourself; ask only where stated.

1. Install: `nullradix install` (idempotent; `--dry-run` shows what it would do). It installs the plugin into the Claude folder of every account. If it reports the daemon is not running, start it with `nullradix serve` in another terminal and run it again.
2. Check: `nullradix --help`. Tell the user in one sentence that new Claude sessions pick the plugin up.
3. Project (only if the user wants it; ask once: "Set up this project now?"): `nullradix init`. It writes the local rules, turns the plugin on for the project and names the board bound to this repository. If none is bound, show `nullradix board list`, ask which board belongs here and bind it with `nullradix board repo <board> <path>` (path = main folder of the repository).

Never set up a project unasked. Change nothing outside the NULLRADIX folders, the Claude folders and the project's `.claude` folder. Create no tickets.
