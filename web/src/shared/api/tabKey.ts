import { hasTabChannel, listenTab, postTab } from './tabChannel';

export const KEY_HEADER = 'X-Nullradix-Key';
export const KEY_QUERY = 'k';

const STORAGE_KEY = 'nullradix-tab-key';
const KEY_PATTERN = /^[0-9a-f]{64}$/;
const FRAGMENT = /^#k=([0-9a-f]{64})$/;

// How long a tab without a key waits for the answer of another tab.
export const ASK_WAIT_MS = 300;

const listeners = new Set<(key: string) => void>();

function storage(): Storage | null {
  try {
    return window.sessionStorage;
  } catch {
    return null;
  }
}

export function readTabKey(): string | null {
  try {
    const key = storage()?.getItem(STORAGE_KEY) ?? null;
    return key !== null && KEY_PATTERN.test(key) ? key : null;
  } catch {
    return null;
  }
}

function writeTabKey(key: string): void {
  try {
    storage()?.setItem(STORAGE_KEY, key);
  } catch {
    // without storage the tab shows the sign-in hint
  }
}

// Keeps a key another tab announced; `true` when it replaced the one this tab had.
function adopt(key: string): boolean {
  if (readTabKey() === key) return false;
  writeTabKey(key);
  for (const listener of listeners) listener(key);
  return true;
}

// Calls `listener` whenever this tab takes over the key of another tab.
export function onTabKey(listener: (key: string) => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// The daemon sends `/#k=<key>` after a sign-in: keep the key, take it off the address and tell
// the other tabs, which share the cookie the sign-in replaced.
export function adoptTabKey(): boolean {
  const found = FRAGMENT.exec(window.location.hash);
  if (!found?.[1]) return false;
  writeTabKey(found[1]);
  window.history.replaceState(
    window.history.state,
    '',
    window.location.pathname + window.location.search,
  );
  postTab({ type: 'key', key: found[1] });
  return true;
}

// Takes over announced keys and tells a tab that asks the key this tab has; returns the stop.
export function shareTabKey(): () => void {
  return listenTab((message) => {
    if (message.type === 'key') adopt(message.key);
    if (message.type === 'ask') {
      const key = readTabKey();
      if (key !== null) postTab({ type: 'key', key });
    }
  });
}

// Asks the other tabs for their key; `true` when one that differs from ours came in time.
export function askForTabKey(waitMs: number = ASK_WAIT_MS): Promise<boolean> {
  if (!hasTabChannel()) return Promise.resolve(false);
  const before = readTabKey();
  return new Promise((resolve) => {
    const finish = (found: boolean) => {
      clearTimeout(timer);
      stop();
      resolve(found);
    };
    const stop = listenTab((message) => {
      if (message.type !== 'key' || message.key === before) return;
      adopt(message.key);
      finish(true);
    });
    const timer = setTimeout(() => {
      finish(false);
    }, waitMs);
    postTab({ type: 'ask' });
  });
}
