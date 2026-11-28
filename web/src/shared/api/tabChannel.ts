export const TAB_CHANNEL = 'nullradix-tabs';

const KEY_PATTERN = /^[0-9a-f]{64}$/;

// What the tabs tell each other: a channel stops at the origin and port.
export type TabMessage =
  | { type: 'ask'; nonce: string }
  | { type: 'answer'; to: string; key: string }
  | { type: 'key'; key: string }
  | { type: 'code'; code: string }
  | { type: 'done'; code: string; ok: boolean };

function isNonce(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0 && value.length <= 64;
}

export function isTabMessage(value: unknown): value is TabMessage {
  if (typeof value !== 'object' || value === null) return false;
  const record = value as Record<string, unknown>;
  switch (record['type']) {
    case 'ask':
      return isNonce(record['nonce']);
    case 'answer':
      return (
        isNonce(record['to']) &&
        typeof record['key'] === 'string' &&
        KEY_PATTERN.test(record['key'])
      );
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

// Calls `handler` for each message of the other tabs and returns the stop.
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

// Tests close the channel to start with a fresh one.
export function closeTabChannel(): void {
  if (channel) channel.close();
  channel = undefined;
}
