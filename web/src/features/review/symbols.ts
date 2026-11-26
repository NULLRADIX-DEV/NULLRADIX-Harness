// Signs of the review, made from code points because sources stay ASCII.
export const ARROW = String.fromCodePoint(0x2192);
export const MINUS = String.fromCodePoint(0x2212);
export const DOT = String.fromCodePoint(0xb7);

export function lines(additions: number, deletions: number): string {
  return `+${String(additions)} ${MINUS}${String(deletions)}`;
}
