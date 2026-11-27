export const KEY_HEADER = 'X-Nullradix-Key';
export const KEY_QUERY = 'k';

const STORAGE_KEY = 'nullradix-tab-key';
const KEY_PATTERN = /^[0-9a-f]{64}$/;
const FRAGMENT = /^#k=([0-9a-f]{64})$/;

// Tabs keep their own copy, so a cookie that leaks to another port is of no use there.
function store(): Storage | null {
  try {
    return window.sessionStorage;
  } catch {
    return null;
  }
}

export function readTabKey(): string | null {
  try {
    const key = store()?.getItem(STORAGE_KEY) ?? null;
    return key !== null && KEY_PATTERN.test(key) ? key : null;
  } catch {
    return null;
  }
}

function writeTabKey(key: string): void {
  try {
    store()?.setItem(STORAGE_KEY, key);
  } catch {
    // without storage the tab shows the sign-in hint
  }
}

// The daemon sends `/#k=<key>` after a sign-in: keep the key and take it off the address.
export function adoptTabKey(): boolean {
  const found = FRAGMENT.exec(window.location.hash);
  if (!found?.[1]) return false;
  writeTabKey(found[1]);
  window.history.replaceState(
    window.history.state,
    '',
    window.location.pathname + window.location.search,
  );
  return true;
}
