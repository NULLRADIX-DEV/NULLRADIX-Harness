import type { QueryClient } from '@tanstack/react-query';
import type { components } from '../api/schema';
import type { ChangeRegistry } from './registry';

export type Change = components['schemas']['ChangeDto'];

export const EVENTS_PATH = '/api/v1/events';
export const RECONNECT_BASE_MS = 1_000;
export const RECONNECT_MAX_MS = 30_000;

export function reconnectDelay(attempt: number): number {
  return Math.min(RECONNECT_BASE_MS * 2 ** attempt, RECONNECT_MAX_MS);
}

export function parseChange(data: unknown): Change | null {
  if (typeof data !== 'string') return null;
  let value: unknown;
  try {
    value = JSON.parse(data);
  } catch {
    return null;
  }
  if (typeof value !== 'object' || value === null) return null;
  const record = value as Record<string, unknown>;
  if (typeof record['seq'] !== 'number' || typeof record['entity'] !== 'string') return null;
  return value as Change;
}

export interface ChangeStreamOptions {
  queryClient: QueryClient;
  registry: ChangeRegistry;
  path?: string;
}

export class ChangeStreamConnection {
  private readonly options: ChangeStreamOptions;
  private source: EventSource | null = null;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private attempt = 0;
  private lastSeq: number | null = null;
  private active = false;

  constructor(options: ChangeStreamOptions) {
    this.options = options;
  }

  open(): void {
    if (this.active) return;
    this.active = true;
    this.connect();
  }

  close(): void {
    this.active = false;
    clearTimeout(this.timer);
    this.timer = undefined;
    this.source?.close();
    this.source = null;
  }

  private connect(): void {
    const path = this.options.path ?? EVENTS_PATH;
    // A new EventSource sends no Last-Event-ID, so resume through the query.
    const url = this.lastSeq === null ? path : `${path}?since=${String(this.lastSeq)}`;
    const source = new EventSource(url);
    this.source = source;
    source.addEventListener('open', () => {
      this.attempt = 0;
    });
    source.addEventListener('ready', (event) => {
      this.track(event);
      this.invalidateAll();
    });
    source.addEventListener('reset', (event) => {
      this.track(event);
      this.invalidateAll();
    });
    source.addEventListener('change', (event) => {
      const change = parseChange(event.data);
      if (!change) return;
      this.lastSeq = change.seq;
      this.invalidateEntity(change.entity);
    });
    source.addEventListener('error', () => {
      if (this.source !== source) return;
      source.close();
      this.source = null;
      this.scheduleReconnect();
    });
  }

  private scheduleReconnect(): void {
    if (!this.active) return;
    const delay = reconnectDelay(this.attempt);
    this.attempt += 1;
    this.timer = setTimeout(() => {
      this.timer = undefined;
      if (this.active) this.connect();
    }, delay);
  }

  private track(event: MessageEvent): void {
    const seq = Number.parseInt(event.lastEventId, 10);
    if (Number.isSafeInteger(seq)) this.lastSeq = seq;
  }

  private invalidateAll(): void {
    void this.options.queryClient.invalidateQueries();
  }

  private invalidateEntity(entity: string): void {
    for (const queryKey of this.options.registry[entity] ?? []) {
      void this.options.queryClient.invalidateQueries({ queryKey });
    }
  }
}
