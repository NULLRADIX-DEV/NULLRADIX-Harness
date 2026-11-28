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

// Whether the daemon accepts the cookie with this key: a bogus key is never kept.
async function vouches(key: string): Promise<boolean> {
  try {
    const url = `${window.location.origin}/api/v1/auth/me`;
    const response = await fetch(new Request(url, { headers: { [KEY_HEADER]: key } }));
    return response.ok;
  } catch {
    return false;
  }
}

// Keeps a key another tab announced and returns true when it replaced the old one.
function adopt(key: string): boolean {
  if (readTabKey() === key) return false;
  writeTabKey(key);
  for (const listener of listeners) listener(key);
  return true;
}

// One check per key at a time: a burst of messages makes one request.
const checking = new Map<string, Promise<boolean>>();

// Keeps a key from another tab once the daemon vouches for it, true when this tab holds it.
function takeChecked(key: string): Promise<boolean> {
  if (readTabKey() === key) return Promise.resolve(true);
  const running = checking.get(key);
  if (running) return running;
  const check = vouches(key)
    .then((good) => {
      if (good) adopt(key);
      return good;
    })
    .finally(() => {
      checking.delete(key);
    });
  checking.set(key, check);
  return check;
}

function newNonce(): string {
  try {
    return crypto.randomUUID();
  } catch {
    return `${Math.random().toString(36).slice(2)}${Date.now().toString(36)}`;
  }
}

// Calls `listener` whenever this tab takes over the key of another tab.
export function onTabKey(listener: (key: string) => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// Clears the address at once, then keeps the key from `/#k=<key>` once the daemon vouches for it.
export function adoptTabKey(): Promise<boolean> {
  const found = FRAGMENT.exec(window.location.hash);
  if (!found?.[1]) return Promise.resolve(false);
  const key = found[1];
  window.history.replaceState(
    window.history.state,
    '',
    window.location.pathname + window.location.search,
  );
  // any page can open the address with a made-up key, and it must not sign the other tabs out
  return vouches(key).then((good) => {
    if (!good) return false;
    writeTabKey(key);
    postTab({ type: 'key', key });
    return true;
  });
}

// Takes over announced keys the daemon vouches for and answers a tab asking for the key.
export function shareTabKey(): () => void {
  return listenTab((message) => {
    if (message.type === 'key') void takeChecked(message.key);
    if (message.type === 'ask') {
      const key = readTabKey();
      if (key !== null) postTab({ type: 'answer', to: message.nonce, key });
    }
  });
}

// Asks the other tabs for their key and returns true when a vouched one came in time.
export function askForTabKey(waitMs: number = ASK_WAIT_MS): Promise<boolean> {
  if (!hasTabChannel()) return Promise.resolve(false);
  const before = readTabKey();
  const nonce = newNonce();
  const started = Date.now();
  return new Promise((resolve) => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const finish = (found: boolean) => {
      clearTimeout(timer);
      stop();
      resolve(found);
    };
    const arm = (ms: number) => {
      timer = setTimeout(() => {
        finish(false);
      }, ms);
    };
    // only answers to this ask count, and a stale key is checked like any other
    const stop = listenTab((message) => {
      if (message.type !== 'answer' || message.to !== nonce || message.key === before) return;
      clearTimeout(timer);
      void takeChecked(message.key).then((held) => {
        if (held) finish(true);
        else arm(Math.max(0, waitMs - (Date.now() - started)));
      });
    });
    arm(waitMs);
    postTab({ type: 'ask', nonce });
  });
}
