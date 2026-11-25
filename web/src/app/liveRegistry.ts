import { accountsLive } from '../features/accounts/queries';
import { boardsLive } from '../features/boards/queries';
import { inboxLive } from '../features/inbox/queries';
import { notesLive } from '../features/notes/queries';
import { projectsLive } from '../features/projects/queries';
import { questionsLive } from '../features/questions/queries';
import { runsLive } from '../features/runs/queries';
import { favoritesLive } from '../features/sessions/favoriteQueries';
import { sessionsLive } from '../features/sessions/queries';
import { ticketsLive } from '../features/tickets/queries';
import { defineChangeRegistry } from '../shared/live/registry';

export const liveRegistry = defineChangeRegistry(
  projectsLive,
  accountsLive,
  boardsLive,
  ticketsLive,
  notesLive,
  questionsLive,
  inboxLive,
  runsLive,
  sessionsLive,
  favoritesLive,
);
