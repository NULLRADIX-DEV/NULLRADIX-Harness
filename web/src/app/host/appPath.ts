// Any origin works as the base: an address that leaves it is not a place of the app.
const BASE = 'http://app.invalid';

// The path, query and hash of an address inside the app, resolved like the browser does; else null.
export function appPathOf(to: unknown): string | null {
  if (typeof to !== 'string' || !to.startsWith('/')) return null;
  let url: URL;
  try {
    url = new URL(to, BASE);
  } catch {
    return null;
  }
  // a path that starts with two slashes is read as a host by whoever follows it
  if (url.origin !== BASE || url.pathname.startsWith('//')) return null;
  return `${url.pathname}${url.search}${url.hash}`;
}
