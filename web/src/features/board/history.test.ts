import { describe, expect, it } from 'vitest';
import type { HistoryLine } from '../tickets/queries';
import { historyView } from './history';

function line(field: string, from: string | null, to: string | null, note?: string): HistoryLine {
  return {
    at: 1,
    actorId: 'a',
    actorName: 'Ben',
    field,
    from,
    to,
    ...(note === undefined ? {} : { note }),
  };
}

const ARROW = String.fromCharCode(0x2192);

describe('historyView', () => {
  it('shows status changes with labels and marks done', () => {
    expect(historyView(line('status', null, 'backlog', 'created'))).toMatchObject({
      title: 'Created in Backlog',
      meta: ['Ben', 'created'],
    });
    expect(historyView(line('status', 'in-progress', 'done'))).toMatchObject({
      title: `In progress ${ARROW} Done`,
      tone: 'ok',
      highlight: true,
    });
  });

  it('shows a block with its reason', () => {
    expect(historyView(line('blockedReason', null, 'API missing'))).toMatchObject({
      title: 'Blocked',
      error: 'API missing',
      tone: 'error',
    });
  });

  it('shows long texts by what happened and lists as text', () => {
    expect(historyView(line('description', '0 characters', '12 characters')).title).toBe(
      'Description written',
    );
    expect(historyView(line('plan', '40 characters', '0 characters')).title).toBe('Plan cleared');
    expect(historyView(line('labels', '["web"]', '["web","ui"]'))).toMatchObject({
      title: 'Labels: web, ui',
      meta: ['Ben', 'before: web'],
    });
  });

  it('shows steps with the note only when it adds something', () => {
    expect(historyView(line('step', '2: open', '2: done', 'Write the page'))).toMatchObject({
      title: 'Step 2: done',
      meta: ['Ben', 'Write the page'],
    });
    expect(historyView(line('step', null, '3: Test', 'step added')).meta).toEqual([
      'Ben',
      'step added',
    ]);
  });

  it('falls back to the field name and both values', () => {
    expect(historyView(line('targetBranch', null, 'main')).title).toBe(
      `Target branch: ${String.fromCharCode(0x2013)} ${ARROW} main`,
    );
  });
});
