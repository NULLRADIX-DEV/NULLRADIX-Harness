export const TAB_CHANNEL = 'nullradix-tabs';

const KEY_PATTERN = /^[0-9a-f]{64}$/;

// What the tabs of this origin tell each other; the channel stops at the origin, port included.
export type TabMessage =
  | { type: 'ask' }
  | { type: 'key'; key: string }
  | { type: 'code'; code: string }
  | { type: 'done'; code: string; ok: boolean };

export function isTabMessage(value: unknown): value is TabMessage {
  if (typeof value !== 'object' || value === null) return false;
  const record = value as Record<string, unknown>;
  switch (record['type']) {
    case 'ask':
      return true;
    case 'key':
      return typeof record['key'] === 'string' && KEY_PATTERN.test(record['key']);
    case 'code':
      return typeof record['code'] === 'string';
    case 'done':
      return typeof record['code'] === 'string' && typeof record['ok'] === 'boolean';
    default:
      return false;
  }
}

let channel: BroadcastChannel | null | undefined;

function open(): BroadcastChannel | null {
  channel ??= typeof BroadcastChannel === 'undefined' ? null : new BroadcastChannel(TAB_CHANNEL);
  return channel;
}

export function hasTabChannel(): boolean {
  return open() !== null;
}

export function postTab(message: TabMessage): void {
  open()?.postMessage(message);
}

// Calls `handler` for every message of the other tabs; returns the way to stop.
export function listenTab(handler: (message: TabMessage) => void): () => void {
  const talk = open();
  if (talk === null) return () => undefined;
  const heard = (event: MessageEvent<unknown>) => {
    if (isTabMessage(event.data)) handler(event.data);
  };
  talk.addEventListener('message', heard);
  return () => {
    talk.removeEventListener('message', heard);
  };
}

// For tests, which swap the channel class.
export function closeTabChannel(): void {
  if (channel) channel.close();
  channel = undefined;
}
