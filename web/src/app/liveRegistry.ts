import { boardsLive } from '../features/boards/queries';
import { notesLive } from '../features/notes/queries';
import { projectsLive } from '../features/projects/queries';
import { questionsLive } from '../features/questions/queries';
import { runsLive } from '../features/runs/queries';
import { ticketsLive } from '../features/tickets/queries';
import { defineChangeRegistry } from '../shared/live/registry';

export const liveRegistry = defineChangeRegistry(
  projectsLive,
  boardsLive,
  ticketsLive,
  notesLive,
  questionsLive,
  runsLive,
);
