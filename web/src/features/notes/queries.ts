import { queryOptions, useMutation } from '@tanstack/react-query';
import { useCallback } from 'react';
import { api, type components } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';
import { useInvalidate } from '../../shared/query/useInvalidate';
import { boardKeys } from '../boards/queries';
import { ticketKeys } from '../tickets/queries';

export type Note = components['schemas']['NoteDto'];
export type NoteState = components['schemas']['NoteStateDto'];
export type CreateNoteInput = components['schemas']['CreateNote'];
export type PromoteNoteInput = components['schemas']['PromoteNote'];

// What a list shows: one state or all of them.
export type NoteFilter = NoteState | 'all';

export const noteKeys = {
  all: ['notes'] as const,
  list: (boardId: string, filter: NoteFilter) =>
    [...noteKeys.all, 'list', boardId, filter] as const,
};

export const notesLive: ChangeRegistry = {
  note: [noteKeys.all],
};

export function notesQuery(projectId: string, boardId: string, filter: NoteFilter) {
  return queryOptions({
    queryKey: noteKeys.list(boardId, filter),
    queryFn: async () =>
      unwrap(
        await api.GET('/api/v1/projects/{projectId}/boards/{boardId}/notes', {
          params: { path: { projectId, boardId }, query: { state: filter } },
        }),
      ).items,
  });
}

// A promotion also adds a ticket and moves the open ticket count of the board.
function useNotesChanged() {
  const notes = useInvalidate(noteKeys.all);
  const tickets = useInvalidate(ticketKeys.all);
  const boards = useInvalidate(boardKeys.all);
  return useCallback(async () => {
    await Promise.all([notes(), tickets(), boards()]);
  }, [notes, tickets, boards]);
}

export interface AddNoteVars {
  projectId: string;
  boardId: string;
  input: CreateNoteInput;
}

export function useAddNote() {
  const changed = useNotesChanged();
  return useMutation({
    mutationFn: async ({ projectId, boardId, input }: AddNoteVars) =>
      unwrap(
        await api.POST('/api/v1/projects/{projectId}/boards/{boardId}/notes', {
          params: { path: { projectId, boardId } },
          body: input,
        }),
      ),
    onSettled: changed,
  });
}

export function useSeenNote() {
  const changed = useNotesChanged();
  return useMutation({
    mutationFn: async (number: number) =>
      unwrap(
        await api.POST('/api/v1/notes/{noteNumber}/seen', {
          params: { path: { noteNumber: String(number) } },
          body: {},
        }),
      ),
    onSettled: changed,
  });
}

export interface PromoteNoteVars {
  number: number;
  input: PromoteNoteInput;
}

export function usePromoteNote() {
  const changed = useNotesChanged();
  return useMutation({
    mutationFn: async ({ number, input }: PromoteNoteVars) =>
      unwrap(
        await api.POST('/api/v1/notes/{noteNumber}/promote', {
          params: { path: { noteNumber: String(number) } },
          body: input,
        }),
      ),
    onSettled: changed,
  });
}

export interface DropNoteVars {
  number: number;
  reason: string;
}

export function useDropNote() {
  const changed = useNotesChanged();
  return useMutation({
    mutationFn: async ({ number, reason }: DropNoteVars) =>
      unwrap(
        await api.POST('/api/v1/notes/{noteNumber}/drop', {
          params: { path: { noteNumber: String(number) } },
          body: { reason },
        }),
      ),
    onSettled: changed,
  });
}
