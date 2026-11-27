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
