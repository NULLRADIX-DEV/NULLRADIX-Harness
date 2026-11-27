import { expect, it } from 'vitest';
import html from '../../index.html?raw';

it('has no inline script or style, so the CSP needs no unsafe-inline', () => {
  const tags = html.match(/<script\b[^>]*>/g) ?? [];
  expect(tags.length).toBeGreaterThan(0);
  for (const tag of tags) expect(tag).toMatch(/\ssrc="/);
  expect(html).not.toMatch(/<script\b[^>]*>\s*(?!<\/script>)\S/);
  expect(html).not.toMatch(/<style/i);
  expect(html).not.toMatch(/\sstyle=/i);
  expect(html).not.toMatch(/\son[a-z]+=/i);
});
