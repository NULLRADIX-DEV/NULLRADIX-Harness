import { screen, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { PROJECT_ID } from '../../test/boardServer';
import { fakeApi, json, problem } from '../../test/fakeApi';
import { renderApp } from '../../test/renderApp';
import type { Question, QuestionDetail } from './queries';

const LIST = `/api/v1/projects/${PROJECT_ID}/questions`;
const DETAIL = '/api/v1/questions/ARC-Q0001';

function question(overrides: Partial<Question> = {}): Question {
  return {
    key: 'ARC-Q0001',
    id: '0190a0b0-0000-7000-8000-00000000f001',
    projectId: PROJECT_ID,
    number: 1,
    title: 'Which store?',
    description: 'Files or a database',
    state: 'open',
    tags: ['storage'],
    tickets: [{ key: 'ARC-0001', title: 'Login page', status: 'backlog' }],
    dependsOn: [],
    waitingOn: [],
    ready: true,
    originName: 'claude',
    originKind: 'session',
    researched: true,
    optionCount: 2,
    createdById: '0190a0b0-0000-7000-8000-0000000000c1',
    createdAt: 1,
    updatedAt: 1,
    version: 1,
    ...overrides,
  };
}

function detail(overrides: Partial<Question> = {}): QuestionDetail {
  return {
    question: question(overrides),
    research: [
      {
        id: '0190a0b0-0000-7000-8000-00000000e001',
        context: '',
        createdAt: 1,
        options: [
          {
            key: 'O1',
            title: 'File',
            text: 'One JSON file',
            pros: ['simple'],
            cons: [],
            recommended: false,
          },
          {
            key: 'O2',
            title: 'Db',
            text: 'SQLite',
            pros: [],
            cons: ['a migration'],
            recommended: true,
          },
        ],
      },
    ],
    comments: [],
  };
}

function server(found: QuestionDetail = detail()) {
  return fakeApi({
    'GET /api/v1/projects': () =>
      json(200, {
        items: [
          { id: PROJECT_ID, key: 'ARC', name: 'Arches', createdAt: 1, updatedAt: 1, version: 1 },
        ],
      }),
    'GET /api/v1/boards': () => json(200, { items: [] }),
    [`GET ${LIST}`]: () =>
      json(200, {
        items: [found.question],
        counts: { open: 1, answered: 2, discarded: 0 },
      }),
    [`GET ${DETAIL}`]: () => json(200, found),
  });
}

describe('QuestionsPage', () => {
  it('lists open questions with counts and answers one by an option', async () => {
    const api = server();
    api.set(`POST ${DETAIL}/answer`, () =>
      json(200, detail({ state: 'answered', version: 2, ready: false })),
    );
    renderApp('/questions');

    const tabs = within(await screen.findByRole('tablist', { name: 'State' }));
    expect(await tabs.findByRole('tab', { name: 'Open (1)' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    expect(tabs.getByRole('tab', { name: 'All (3)' })).toBeInTheDocument();
    expect(api.requests(`GET ${LIST}`)[0]?.url).toContain('state=open');

    await userEvent.click(await screen.findByRole('button', { name: /ARC-Q0001/ }));
    const panel = within(await screen.findByRole('article', { name: 'Question ARC-Q0001' }));
    expect(panel.getByText('Recommended')).toBeInTheDocument();
    expect(panel.getByText('+ simple')).toBeInTheDocument();
    await userEvent.click(panel.getByRole('radio', { name: /Db/ }));
    await userEvent.type(panel.getByRole('textbox', { name: 'Reason' }), 'Fits');
    await userEvent.click(panel.getByRole('button', { name: 'Answer: O2' }));

    const [sent] = api.requests(`POST ${DETAIL}/answer`);
    expect(await sent?.json()).toEqual({ option: 'O2', version: 1, reason: 'Fits' });
  });

  it('shows why a decision was refused', async () => {
    const api = server();
    api.set(`POST ${DETAIL}/answer`, () => problem(422, 'human-only', 'a person decides'));
    renderApp('/questions?question=ARC-Q0001');

    const panel = within(await screen.findByRole('article', { name: 'Question ARC-Q0001' }));
    await userEvent.type(panel.getByRole('textbox', { name: 'Own answer' }), 'Neither');
    await userEvent.click(panel.getByRole('button', { name: 'Answer' }));

    expect(await panel.findByRole('alert')).toHaveTextContent('Only a person decides a question.');
    const [sent] = api.requests(`POST ${DETAIL}/answer`);
    expect(await sent?.json()).toEqual({ text: 'Neither', version: 1 });
  });

  it('shows a decided question with its decision and reopens it', async () => {
    const decided = detail({
      state: 'discarded',
      ready: false,
      decision: { reason: 'moot', byName: 'Ben', at: 2, autopilot: false },
    });
    const api = server(decided);
    api.set(`POST ${DETAIL}/reopen`, () => json(200, detail({ version: 3 })));
    renderApp('/questions?question=ARC-Q0001');

    const panel = within(await screen.findByRole('article', { name: 'Question ARC-Q0001' }));
    expect(panel.getByText('Reason: moot')).toBeInTheDocument();
    expect(panel.queryByRole('radiogroup')).not.toBeInTheDocument();
    await userEvent.click(panel.getByRole('button', { name: 'Reopen' }));

    const [sent] = api.requests(`POST ${DETAIL}/reopen`);
    expect(await sent?.json()).toEqual({ version: 1 });
  });

  it('adds a comment and empties the box', async () => {
    const api = server();
    api.set(`POST ${DETAIL}/comments`, () =>
      json(201, {
        id: '0190a0b0-0000-7000-8000-00000000d001',
        questionKey: 'ARC-Q0001',
        authorId: '0190a0b0-0000-7000-8000-0000000000c1',
        authorName: 'Ben',
        authorKind: 'human',
        body: 'Looks good',
        createdAt: 3,
      }),
    );
    renderApp('/questions?question=ARC-Q0001');

    const panel = within(await screen.findByRole('article', { name: 'Question ARC-Q0001' }));
    const box = panel.getByRole('textbox', { name: 'New comment' });
    await userEvent.type(box, 'Looks good');
    await userEvent.click(panel.getByRole('button', { name: 'Comment' }));

    const [sent] = api.requests(`POST ${DETAIL}/comments`);
    expect(await sent?.json()).toEqual({ body: 'Looks good' });
    expect(box).toHaveValue('');
  });
});
