import { QueryClient } from '@tanstack/react-query';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { FakeEventSource } from '../../test/eventSource';
import {
  ChangeStreamConnection,
  RECONNECT_MAX_MS,
  parseChange,
  reconnectDelay,
} from './connection';
import { defineChangeRegistry } from './registry';

const registry = defineChangeRegistry(
  { project: [['projects']] },
  { project: [['boards', 'list']], ticket: [['tickets']] },
);

function change(seq: number, entity = 'project') {
  return { seq, entity, entityId: '0190a0b0-0000-7000-8000-000000000001', op: 'update', at: 1 };
}

function setup() {
  const queryClient = new QueryClient();
  const invalidate = vi.spyOn(queryClient, 'invalidateQueries').mockResolvedValue(undefined);
  const onError = vi.fn();
  const connection = new ChangeStreamConnection({ queryClient, registry, onError });
  connection.open();
  return { invalidate, onError, connection, source: FakeEventSource.latest() };
}

describe('defineChangeRegistry', () => {
  it('merges the key prefixes of every feature', () => {
    expect(registry).toEqual({
      project: [['projects'], ['boards', 'list']],
      ticket: [['tickets']],
    });
  });
});

describe('parseChange', () => {
  it('accepts change payloads and rejects anything else', () => {
    expect(parseChange(JSON.stringify(change(3)))).toEqual(change(3));
    expect(parseChange('not json')).toBeNull();
    expect(parseChange(JSON.stringify({ seq: 'x', entity: 'project' }))).toBeNull();
    expect(parseChange(42)).toBeNull();
  });
});

describe('reconnectDelay', () => {
  it('doubles up to the maximum', () => {
    expect([0, 1, 2, 3].map(reconnectDelay)).toEqual([1_000, 2_000, 4_000, 8_000]);
    expect(reconnectDelay(20)).toBe(RECONNECT_MAX_MS);
  });
});

describe('ChangeStreamConnection', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('opens one stream on the events route', () => {
    const { connection, source } = setup();
    connection.open();
    expect(FakeEventSource.instances).toHaveLength(1);
    expect(source.url).toBe('/api/v1/events');
  });

  it('invalidates everything when the stream is ready', () => {
    const { invalidate, source } = setup();
    source.open();
    source.emit('ready', { seq: 4 }, '4');
    expect(invalidate).toHaveBeenCalledOnce();
    expect(invalidate).toHaveBeenCalledWith();
  });

  it('invalidates the registered prefixes of a changed entity', () => {
    const { invalidate, source } = setup();
    source.emit('change', change(5), '5');
    expect(invalidate.mock.calls).toEqual([
      [{ queryKey: ['projects'] }],
      [{ queryKey: ['boards', 'list'] }],
    ]);
  });

  it('ignores malformed changes', () => {
    const { invalidate, source } = setup();
    source.dispatchEvent(new MessageEvent('change', { data: '{', lastEventId: '6' }));
    expect(invalidate).not.toHaveBeenCalled();
  });

  it('warns about changes of unregistered entities in dev builds', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const { invalidate, source } = setup();
    source.emit('change', change(5, 'runner'), '5');
    expect(invalidate).not.toHaveBeenCalled();
    expect(warn).toHaveBeenCalledWith('change stream: no query keys registered for runner');
  });

  it('reports every stream error', () => {
    const { onError, source } = setup();
    source.fail();
    expect(onError).toHaveBeenCalledOnce();
    vi.advanceTimersByTime(1_000);
    FakeEventSource.latest().fail();
    expect(onError).toHaveBeenCalledTimes(2);
  });

  it('resets the backoff after a successful open', () => {
    const { source } = setup();
    source.fail();
    vi.advanceTimersByTime(1_000);
    FakeEventSource.latest().fail();
    vi.advanceTimersByTime(2_000);
    expect(FakeEventSource.instances).toHaveLength(3);

    const opened = FakeEventSource.latest();
    opened.open();
    opened.fail();
    vi.advanceTimersByTime(999);
    expect(FakeEventSource.instances).toHaveLength(3);
    vi.advanceTimersByTime(1);
    expect(FakeEventSource.instances).toHaveLength(4);
  });

  it('invalidates everything on reset', () => {
    const { invalidate, source } = setup();
    source.emit('reset', {}, '9');
    expect(invalidate).toHaveBeenCalledWith();
  });

  it('reconnects with backoff and resumes after the last seen event', () => {
    const { source } = setup();
    source.emit('ready', { seq: 4 }, '4');
    source.emit('change', change(7), '7');

    source.fail();
    expect(source.readyState).toBe(FakeEventSource.CLOSED);
    vi.advanceTimersByTime(999);
    expect(FakeEventSource.instances).toHaveLength(1);
    vi.advanceTimersByTime(1);
    const second = FakeEventSource.latest();
    expect(second.url).toBe('/api/v1/events?since=7');

    second.fail();
    vi.advanceTimersByTime(1_999);
    expect(FakeEventSource.instances).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(FakeEventSource.instances).toHaveLength(3);
  });

  it('closes the stream and cancels a pending reconnect', () => {
    const { connection, source } = setup();
    source.fail();
    connection.close();
    vi.advanceTimersByTime(RECONNECT_MAX_MS);
    expect(FakeEventSource.instances).toHaveLength(1);

    connection.open();
    const next = FakeEventSource.latest();
    connection.close();
    expect(next.readyState).toBe(FakeEventSource.CLOSED);
  });
});
