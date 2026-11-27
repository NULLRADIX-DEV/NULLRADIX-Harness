// The link to the active CSS snippets; it sits after the theme stylesheet so snippets win.
export const SNIPPETS_LINK_ID = 'nullradix-snippets-css';

export const SNIPPETS_URL = '/api/v1/snippets/active.css';

export const SNIPPET_ID_MAX = 40;

// Letters with a German spelling, and the marks NFKD splits off other letters.
const AE = new RegExp(String.fromCodePoint(0xe4), 'g');
const OE = new RegExp(String.fromCodePoint(0xf6), 'g');
const UE = new RegExp(String.fromCodePoint(0xfc), 'g');
const SS = new RegExp(String.fromCodePoint(0xdf), 'g');
const MARKS = new RegExp(`[${String.fromCodePoint(0x300)}-${String.fromCodePoint(0x36f)}]`, 'g');

interface SnippetStamp {
  id: string;
  version: number;
  enabled: boolean;
}

// Changes whenever the stylesheet would; null when no snippet is on.
export function snippetsVersion(snippets: readonly SnippetStamp[]): string | null {
  const active = snippets.filter((snippet) => snippet.enabled);
  if (active.length === 0) return null;
  return active.map((snippet) => `${snippet.id}.${String(snippet.version)}`).join(',');
}

// Turns a name into an id (a-z, 0-9, hyphen) that is not taken yet.
export function snippetId(name: string, taken: Iterable<string>): string {
  const used = new Set(taken);
  const slug = name
    .toLowerCase()
    .replace(AE, 'ae')
    .replace(OE, 'oe')
    .replace(UE, 'ue')
    .replace(SS, 'ss')
    .normalize('NFKD')
    .replace(MARKS, '')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, SNIPPET_ID_MAX)
    .replace(/-+$/, '');
  const base = slug.length >= 2 ? slug : 'snippet';
  if (!used.has(base)) return base;
  for (let count = 2; ; count += 1) {
    const suffix = `-${String(count)}`;
    const candidate = `${base.slice(0, SNIPPET_ID_MAX - suffix.length)}${suffix}`;
    if (!used.has(candidate)) return candidate;
  }
}

// Shows the stylesheet of the given version, or none; the old one stays until the new one loaded.
export function applySnippetsStylesheet(doc: Document, version: string | null): void {
  const existing = doc.getElementById(SNIPPETS_LINK_ID);
  if (version === null) {
    existing?.remove();
    return;
  }
  const href = `${SNIPPETS_URL}?v=${version}`;
  if (existing instanceof HTMLLinkElement && existing.getAttribute('href') === href) return;
  const link = doc.createElement('link');
  link.rel = 'stylesheet';
  link.href = href;
  if (!existing) {
    link.id = SNIPPETS_LINK_ID;
    doc.head.appendChild(link);
    return;
  }
  const drop = () => {
    existing.remove();
  };
  link.addEventListener('load', drop, { once: true });
  link.addEventListener('error', drop, { once: true });
  existing.removeAttribute('id');
  link.id = SNIPPETS_LINK_ID;
  existing.after(link);
}
