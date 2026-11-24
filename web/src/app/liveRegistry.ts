import { projectsLive } from '../features/projects/queries';
import { defineChangeRegistry } from '../shared/live/registry';

export const liveRegistry = defineChangeRegistry(projectsLive);
