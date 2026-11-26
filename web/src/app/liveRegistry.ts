import { accountsLive } from '../features/accounts/queries';
import { boardsLive } from '../features/boards/queries';
import { dispatcherLive } from '../features/dispatcher/queries';
import { inboxLive } from '../features/inbox/queries';
import { notesLive } from '../features/notes/queries';
import { projectsLive } from '../features/projects/queries';
import { questionsLive } from '../features/questions/queries';
import { reviewLive } from '../features/review/queries';
import { runsLive } from '../features/runs/queries';
import { catalogLive } from '../shared/query/themeCatalog';
import { themesLive } from '../shared/query/themes';
import { customizationsLive } from '../features/customizations/queries';
import { favoritesLive } from '../features/sessions/favoriteQueries';
import { sessionsLive } from '../features/sessions/queries';
import { ticketsLive } from '../features/tickets/queries';
import { defineChangeRegistry } from '../shared/live/registry';

export const liveRegistry = defineChangeRegistry(
  projectsLive,
  accountsLive,
  dispatcherLive,
  boardsLive,
  ticketsLive,
  notesLive,
  questionsLive,
  inboxLive,
  runsLive,
  reviewLive,
  sessionsLive,
  favoritesLive,
  themesLive,
  catalogLive,
  customizationsLive,
);
