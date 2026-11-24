import type { Board } from '../features/boards/queries';
import type {
  HistoryLine,
  Step,
  Ticket,
  TicketChange,
  TicketDetail,
  TicketStatus,
} from '../features/tickets/queries';
import { fakeApi, json, problem } from './fakeApi';

export const PROJECT_ID = '0190a0b0-0000-7000-8000-000000000001';
export const BOARD_ID = '0190a0b0-0000-7000-8000-0000000000b1';
export const USER_ID = '0190a0b0-0000-7000-8000-0000000000c1';
// Steps routes exist for this many step numbers per ticket.
const STEP_ROUTES = 6;

export function board(overrides: Partial<Board> = {}): Board {
  return {
    id: BOARD_ID,
    projectId: PROJECT_ID,
    projectKey: 'ARC',
    name: 'Main',
    ownerId: USER_ID,
    ownerName: 'Ben',
    dispatchEnabled: false,
    openTickets: 0,
    createdAt: 1,
    updatedAt: 1,
    version: 1,
    ...overrides,
  };
}

export function ticket(overrides: Partial<Ticket> = {}): Ticket {
  const number = overrides.number ?? 1;
  return {
    key: `ARC-${String(number).padStart(4, '0')}`,
    id: `0190a0b0-0000-7000-8000-${String(number).padStart(12, '0')}`,
    projectId: PROJECT_ID,
    boardId: BOARD_ID,
    number,
    title: 'Show the board',
    description: '',
    acceptance: '',
    plan: '',
    handbook: '',
    diagram: '',
    status: 'backlog',
    priority: 'normal',
    labels: [],
    dependsOn: [],
    needsPlanning: false,
    createdById: USER_ID,
    createdByName: 'Ben',
    createdAt: 1,
    updatedAt: 1,
    version: 1,
    column: 'backlog',
    waiting: false,
    dependsOpen: [],
    dependents: [],
    unlocks: [],
    stepsDone: 0,
    stepsTotal: 0,
    ...overrides,
  };
}

export interface Entry {
  ticket: Ticket;
  steps: Step[];
  history: HistoryLine[];
}

export function entry(base: Ticket, steps: Step[] = [], history: HistoryLine[] = []): Entry {
  return { ticket: base, steps, history };
}

function line(field: string, from: string | null, to: string | null): HistoryLine {
  return { at: Date.now(), actorId: USER_ID, actorName: 'Ben', field, from, to };
}

function urlOf(request: Request): URL {
  return new URL(request.url);
}

// In-memory daemon for the board, ticket and step routes.
export function boardServer(
  options: { boards?: Board[]; entries?: Entry[] } = {},
): ReturnType<typeof setup> {
  return setup(options.boards ?? [board()], options.entries ?? [entry(ticket())]);
}

function setup(boards: Board[], initial: Entry[]) {
  const entries = new Map(initial.map((item) => [item.ticket.key, item]));
  const api = fakeApi({
    'GET /api/v1/boards': () => json(200, { items: boards }),
    'GET /api/v1/tickets': (request) => {
      const boardId = urlOf(request).searchParams.get('boardId');
      const items = [...entries.values()]
        .map((item) => item.ticket)
        .filter((item) => boardId === null || item.boardId === boardId);
      return json(200, { items });
    },
  });

  function detail(item: Entry): TicketDetail {
    return { ticket: item.ticket, steps: item.steps, history: item.history };
  }

  function revise(item: Entry, change: Partial<Ticket>, history: HistoryLine[] = []) {
    const steps = item.steps;
    item.ticket = {
      ...item.ticket,
      ...change,
      version: item.ticket.version + 1,
      stepsDone: steps.filter((step) => step.done).length,
      stepsTotal: steps.length,
    };
    item.history = [...item.history, ...history];
  }

  function find(key: string, expected: number | null): Entry | Response {
    const item = entries.get(key);
    if (!item) return problem(404, 'ticket-not-found', `ticket ${key} does not exist`);
    if (expected !== null && expected !== item.ticket.version) {
      return problem(409, 'version-conflict', 'the ticket changed');
    }
    return item;
  }

  function stepAt(item: Entry, number: number, expectText: string | null | undefined) {
    const step = item.steps[number - 1];
    if (!step) return problem(404, 'step-not-found', `no step ${String(number)}`);
    if (expectText !== undefined && expectText !== null && step.text !== expectText) {
      return problem(409, 'conflict', 'the step changed');
    }
    return step;
  }

  function register(key: string) {
    const base = `/api/v1/tickets/${key}`;
    api.set(`GET ${base}`, () => {
      const item = find(key, null);
      return item instanceof Response ? item : json(200, detail(item));
    });
    api.set(`PATCH ${base}`, async (request) => {
      const body = (await request.json()) as TicketChange & { version: number };
      const item = find(key, body.version);
      if (item instanceof Response) return item;
      const change = Object.entries(body).filter(([field]) => field !== 'version');
      const history = change.map(([field, value]) => line(field, null, JSON.stringify(value)));
      revise(item, Object.fromEntries(change), history);
      return json(200, detail(item));
    });
    api.set(`POST ${base}/move`, async (request) => {
      const body = (await request.json()) as {
        status: TicketStatus;
        version?: number;
        acceptance?: string;
      };
      const item = find(key, body.version ?? null);
      if (item instanceof Response) return item;
      const acceptance = body.acceptance ?? item.ticket.acceptance;
      if (body.status === 'done' && acceptance.trim() === '') {
        return problem(422, 'acceptance-missing', 'done needs acceptance criteria');
      }
      if (body.status === 'done' && item.ticket.dependsOpen.length > 0) {
        return problem(422, 'dependency-open', 'a dependency is open');
      }
      revise(item, { status: body.status, column: body.status, acceptance }, [
        line('status', item.ticket.status, body.status),
      ]);
      return json(200, item.ticket);
    });
    api.set(`DELETE ${base}`, (request) => {
      const item = find(key, Number(urlOf(request).searchParams.get('version')));
      if (item instanceof Response) return item;
      entries.delete(key);
      return new Response(null, { status: 204 });
    });
    api.set(`POST ${base}/steps`, async (request) => {
      const body = (await request.json()) as { text: string; detail?: string };
      const item = find(key, null);
      if (item instanceof Response) return item;
      item.steps = [...item.steps, { number: item.steps.length + 1, text: body.text, done: false }];
      revise(item, {}, [line('step', null, `${String(item.steps.length)}: ${body.text}`)]);
      return json(201, detail(item));
    });
    for (let number = 1; number <= STEP_ROUTES; number += 1) {
      api.set(`PATCH ${base}/steps/${String(number)}`, async (request) => {
        const body = (await request.json()) as {
          done?: boolean;
          text?: string;
          expectText?: string;
        };
        const item = find(key, null);
        if (item instanceof Response) return item;
        const step = stepAt(item, number, body.expectText);
        if (step instanceof Response) return step;
        const changed = {
          ...step,
          ...(body.done === undefined ? {} : { done: body.done }),
          ...(body.text === undefined ? {} : { text: body.text }),
        };
        item.steps = item.steps.map((each, index) => (index === number - 1 ? changed : each));
        revise(item, {}, [line('step', null, `${String(number)}: ${changed.text}`)]);
        return json(200, detail(item));
      });
    }
  }

  for (const key of entries.keys()) register(key);

  api.set(`POST /api/v1/projects/${PROJECT_ID}/boards/${BOARD_ID}/tickets`, async (request) => {
    const body = (await request.json()) as {
      title: string;
      labels?: string[];
      allowDuplicate?: boolean;
    };
    const existing = [...entries.values()].find((item) => item.ticket.title === body.title);
    if (existing && !body.allowDuplicate) {
      const response = {
        type: 'about:blank',
        title: 'Conflict',
        status: 409,
        code: 'ticket-duplicate',
        detail: 'taken',
        existing: existing.ticket.key,
      };
      return new Response(JSON.stringify(response), {
        status: 409,
        headers: { 'content-type': 'application/problem+json' },
      });
    }
    const created = ticket({
      number: entries.size + 1,
      title: body.title,
      labels: body.labels ?? [],
    });
    entries.set(created.key, entry(created));
    register(created.key);
    return json(201, created);
  });

  return {
    api,
    entries,
    // Simulates a change by another client.
    bumpElsewhere(key: string, change: Partial<Ticket>) {
      const item = entries.get(key);
      if (item) revise(item, change);
    },
  };
}
