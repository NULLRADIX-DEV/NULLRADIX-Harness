# NULLRADIX board rule

- Tickets, board, kanban, columns, backlog, planning, questions, notes and keys like `ARC-42` or `ARC-Q7` always mean the NULLRADIX board: use `nullradix ticket ...`, `nullradix question ...` and `nullradix note ...`; details are in the board and question skills.
- Never use another kanban tool, GitHub issues or GitLab issues. Read Azure DevOps work items only when asked.
- A repository works on the board bound to its folder (`nullradix board repo <board> <path>`), else on the active board (`nullradix board use <board>`).
- A ticket needs acceptance criteria before it is done. A missing decision becomes a question, not a blocked reason. A finding about something else becomes a note.
- Autopilot (dispatcher setting): agents of the dispatcher get neither a review before the merge nor the outcome `Human Interaction`; research decides their questions, and dead ends block the ticket. Answer questions of people and interactive sessions never.
