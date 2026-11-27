import { afterEach, describe, expect, it } from 'vitest';
import { THEME_LINK_ID, applyInstalledTheme } from './theme';
import {
  SNIPPETS_LINK_ID,
  SNIPPETS_URL,
  applySnippetsStylesheet,
  snippetId,
  snippetsVersion,
} from './snippets';

afterEach(() => {
  document.head.replaceChildren();
});

function links(): HTMLLinkElement[] {
  return [...document.head.querySelectorAll('link')];
}

const o = String.fromCodePoint(0xf6);
const ss = String.fromCodePoint(0xdf);
const e = String.fromCodePoint(0xe9);

describe('snippetId', () => {
  it('turns a name into a short id', () => {
    expect(snippetId('Red Links', [])).toBe('red-links');
    expect(snippetId(`  Gr${o}${ss}e: gro${ss}!  `, [])).toBe('groesse-gross');
    expect(snippetId(`caf${e}`, [])).toBe('cafe');
    expect(snippetId('x'.repeat(60), [])).toHaveLength(40);
  });

  it('falls back for names without letters and avoids ids in use', () => {
    expect(snippetId('!!', [])).toBe('snippet');
    expect(snippetId('a', [])).toBe('snippet');
    expect(snippetId('Red', ['red'])).toBe('red-2');
    expect(snippetId('Red', ['red', 'red-2'])).toBe('red-3');
    const long = 'x'.repeat(40);
    const next = snippetId(long, [long]);
    expect(next).toHaveLength(40);
    expect(next.endsWith('-2')).toBe(true);
  });
});

describe('snippetsVersion', () => {
  it('is null without enabled snippets and changes with every edit', () => {
    expect(snippetsVersion([])).toBeNull();
    expect(snippetsVersion([{ id: 'a1', version: 1, createdAt: 1, enabled: false }])).toBeNull();
    const one = [{ id: 'a1', version: 1, createdAt: 1, enabled: true }];
    const edited = [{ id: 'a1', version: 2, createdAt: 1, enabled: true }];
    const added = [...one, { id: 'b2', version: 1, createdAt: 1, enabled: true }];
    const versions = [one, edited, added].map(snippetsVersion);
    expect(new Set(versions).size).toBe(3);
  });

  it('differs for a snippet deleted and created again under the same id', () => {
    const first = [{ id: 'a1', version: 1, createdAt: 5, enabled: true }];
    const again = [{ id: 'a1', version: 1, createdAt: 9, enabled: true }];
    expect(snippetsVersion(first)).not.toBe(snippetsVersion(again));
  });
});

describe('applySnippetsStylesheet', () => {
  it('adds one link for the active snippets and none when nothing is on', () => {
    applySnippetsStylesheet(document, 'a1.1');
    applySnippetsStylesheet(document, 'a1.1');
    expect(links()).toHaveLength(1);
    expect(document.getElementById(SNIPPETS_LINK_ID)?.getAttribute('href')).toBe(
      `${SNIPPETS_URL}?v=a1.1`,
    );
    applySnippetsStylesheet(document, null);
    expect(links()).toHaveLength(0);
  });

  it('keeps the old stylesheet until the new one has loaded', () => {
    applySnippetsStylesheet(document, 'a1.1');
    const old = document.getElementById(SNIPPETS_LINK_ID);
    applySnippetsStylesheet(document, 'a1.2');
    const current = document.getElementById(SNIPPETS_LINK_ID);
    expect(current?.getAttribute('href')).toBe(`${SNIPPETS_URL}?v=a1.2`);
    expect(links()).toHaveLength(2);
    current?.dispatchEvent(new Event('load'));
    expect(links()).toEqual([current]);
    expect(old?.isConnected).toBe(false);
  });

  it('keeps what is still needed when changes follow each other quickly', () => {
    applySnippetsStylesheet(document, 'a1.1');
    applySnippetsStylesheet(document, 'a1.2');
    applySnippetsStylesheet(document, 'a1.3');
    const [first, second, third] = links();
    expect(links()).toHaveLength(3);
    expect(document.querySelectorAll(`#${SNIPPETS_LINK_ID}`)).toHaveLength(1);
    expect(third?.id).toBe(SNIPPETS_LINK_ID);
    // the middle one loads first: only the oldest goes, the newest still waits
    second?.dispatchEvent(new Event('load'));
    expect(links()).toEqual([second, third]);
    third?.dispatchEvent(new Event('load'));
    expect(links()).toEqual([third]);
    expect(first?.isConnected).toBe(false);
  });

  it('drops every pending link when nothing is on any more', () => {
    applySnippetsStylesheet(document, 'a1.1');
    applySnippetsStylesheet(document, 'a1.2');
    applySnippetsStylesheet(document, null);
    expect(links()).toHaveLength(0);
    applySnippetsStylesheet(document, 'a1.3');
    expect(links()).toHaveLength(1);
    expect(document.getElementById(SNIPPETS_LINK_ID)?.getAttribute('href')).toBe(
      `${SNIPPETS_URL}?v=a1.3`,
    );
  });

  it('does not add a link again for the version that is the newest', () => {
    applySnippetsStylesheet(document, 'a1.1');
    applySnippetsStylesheet(document, 'a1.2');
    applySnippetsStylesheet(document, 'a1.2');
    expect(links()).toHaveLength(2);
  });

  it('lets a failed load drop the old stylesheet too', () => {
    applySnippetsStylesheet(document, 'a1.1');
    applySnippetsStylesheet(document, 'a1.2');
    document.getElementById(SNIPPETS_LINK_ID)?.dispatchEvent(new Event('error'));
    expect(links()).toHaveLength(1);
  });

  it('stays after the theme stylesheet, whichever comes first', () => {
    const look = {
      id: 'night-blue',
      scheme: 'dark' as const,
      styleUrl: '/theme.css',
      startColor: null,
    };
    applySnippetsStylesheet(document, 'a1.1');
    applyInstalledTheme(document.documentElement, look);
    expect(links().map((link) => link.id)).toEqual([THEME_LINK_ID, SNIPPETS_LINK_ID]);
    applyInstalledTheme(document.documentElement, { ...look, styleUrl: '/other.css' });
    expect(links().map((link) => link.id)).toEqual([THEME_LINK_ID, SNIPPETS_LINK_ID]);
  });
});
