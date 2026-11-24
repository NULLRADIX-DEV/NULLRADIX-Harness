import { projectsLive } from '../features/projects/queries';
import { runsLive } from '../features/runs/queries';
import { defineChangeRegistry } from '../shared/live/registry';

export const liveRegistry = defineChangeRegistry(projectsLive, runsLive);
