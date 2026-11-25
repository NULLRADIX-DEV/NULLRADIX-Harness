import { queryOptions, useMutation, useQueryClient } from '@tanstack/react-query';
import { useCallback } from 'react';
import { api, type components } from '../../shared/api/client';
import { unwrap } from '../../shared/api/errors';
import type { ChangeRegistry } from '../../shared/live/registry';
import { useInvalidate } from '../../shared/query/useInvalidate';
import { ticketKeys } from '../tickets/queries';

export type Question = components['schemas']['QuestionDto'];
export type QuestionDetail = components['schemas']['QuestionDetailDto'];
export type QuestionState = components['schemas']['QuestionStateDto'];
export type QuestionOption = components['schemas']['OptionDto'];
export type QuestionComment = components['schemas']['QuestionCommentDto'];
export type QuestionCounts = components['schemas']['QuestionCounts'];
export type StateFilter = QuestionState | 'all';

export const questionKeys = {
  all: ['questions'] as const,
  lists: () => [...questionKeys.all, 'list'] as const,
  list: (projectId: string, state: StateFilter) =>
    [...questionKeys.lists(), projectId, state] as const,
  detail: (key: string) => [...questionKeys.all, 'detail', key] as const,
};

// A decision frees or holds tickets, so their cards and panels refetch with the question.
export const questionsLive: ChangeRegistry = {
  question: [questionKeys.all, ticketKeys.all],
  comment: [questionKeys.all],
};

export function questionsQuery(projectId: string, state: StateFilter) {
  return queryOptions({
    queryKey: questionKeys.list(projectId, state),
    queryFn: async () =>
      unwrap(
        await api.GET('/api/v1/projects/{projectId}/questions', {
          params: { path: { projectId }, query: { state } },
        }),
      ),
  });
}

export function questionQuery(key: string) {
  return queryOptions({
    queryKey: questionKeys.detail(key),
    queryFn: async () =>
      unwrap(
        await api.GET('/api/v1/questions/{questionKey}', {
          params: { path: { questionKey: key } },
        }),
      ),
  });
}

function useQuestionsChanged() {
  const questions = useInvalidate(questionKeys.all);
  const tickets = useInvalidate(ticketKeys.all);
  return useCallback(async () => {
    await Promise.all([questions(), tickets()]);
  }, [questions, tickets]);
}

// Stores the answer so the next decision starts from its version before the refetch lands.
function useDecision<Vars>(mutationFn: (vars: Vars) => Promise<QuestionDetail>) {
  const queryClient = useQueryClient();
  const changed = useQuestionsChanged();
  return useMutation({
    mutationFn,
    onSuccess: (detail) => {
      queryClient.setQueryData(questionKeys.detail(detail.question.key), detail);
    },
    onSettled: changed,
  });
}

export interface AnswerVars {
  key: string;
  version: number;
  option?: string;
  text?: string;
  reason?: string;
}

export function useAnswerQuestion() {
  return useDecision(async ({ key, ...body }: AnswerVars) =>
    unwrap(
      await api.POST('/api/v1/questions/{questionKey}/answer', {
        params: { path: { questionKey: key } },
        body,
      }),
    ),
  );
}

export interface DecideVars {
  key: string;
  version: number;
  reason?: string;
}

export function useDiscardQuestion() {
  return useDecision(async ({ key, ...body }: DecideVars) =>
    unwrap(
      await api.POST('/api/v1/questions/{questionKey}/discard', {
        params: { path: { questionKey: key } },
        body,
      }),
    ),
  );
}

export function useReopenQuestion() {
  return useDecision(async ({ key, ...body }: DecideVars) =>
    unwrap(
      await api.POST('/api/v1/questions/{questionKey}/reopen', {
        params: { path: { questionKey: key } },
        body,
      }),
    ),
  );
}

export interface QuestionCommentVars {
  key: string;
  body: string;
}

export function useAddQuestionComment() {
  const changed = useQuestionsChanged();
  return useMutation({
    mutationFn: async ({ key, body }: QuestionCommentVars) =>
      unwrap(
        await api.POST('/api/v1/questions/{questionKey}/comments', {
          params: { path: { questionKey: key } },
          body: { body },
        }),
      ),
    onSettled: changed,
  });
}
