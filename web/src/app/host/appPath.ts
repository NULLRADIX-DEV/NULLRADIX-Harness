// Any origin works as the base, as only the part after it is used.
const BASE = 'http://app.invalid';

// Path, query and hash of an address inside the app, else null.
export function appPathOf(to: unknown): string | null {
  if (typeof to !== 'string' || !to.startsWith('/')) return null;
  let url: URL;
  try {
    url = new URL(to, BASE);
  } catch {
    return null;
  }
  // Two leading slashes would be read as a host by the browser.
  if (url.origin !== BASE || url.pathname.startsWith('//')) return null;
  return `${url.pathname}${url.search}${url.hash}`;
}
