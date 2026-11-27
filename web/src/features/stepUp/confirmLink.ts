export const CONFIRM_CHANNEL = 'nullradix-step-up';

const FRAGMENT = /^#confirm=([0-9a-f]{64})$/;

// The code a terminal or the tray menu left in the address, if any.
export function parseConfirmCode(hash: string): string | null {
  return FRAGMENT.exec(hash)?.[1] ?? null;
}

function clearFragment() {
  window.history.replaceState(
    window.history.state,
    '',
    window.location.pathname + window.location.search,
  );
}

// Reads the code out of the address and removes it, before the router can navigate away.
export function takeConfirmCode(): string | null {
  const code = parseConfirmCode(window.location.hash);
  if (code !== null) clearFragment();
  return code;
}

// What tabs tell each other: a tab opened from a terminal has no key and hands its code on.
export type ConfirmMessage =
  { type: 'code'; code: string } | { type: 'done'; code: string; ok: boolean };

export function isConfirmMessage(value: unknown): value is ConfirmMessage {
  if (typeof value !== 'object' || value === null) return false;
  const record = value as Record<string, unknown>;
  if (typeof record['code'] !== 'string') return false;
  if (record['type'] === 'code') return true;
  return record['type'] === 'done' && typeof record['ok'] === 'boolean';
}
