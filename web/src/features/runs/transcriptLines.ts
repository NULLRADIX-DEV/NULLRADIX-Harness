import type { TranscriptLine } from './dispatchQueries';

export type CallOutcome = 'ok' | 'fail';

export interface TranscriptItem {
  line: TranscriptLine;
  // How the call ended, when its result line is known.
  outcome: CallOutcome | null;
  // Text of a failed result, shown beside the call.
  failure: string | null;
}

function isResult(line: TranscriptLine): boolean {
  return line.kind === 'ok' || line.kind === 'fail';
}

// Results with a reference fold into their call, which then takes the color of the result.
export function foldResults(lines: readonly TranscriptLine[]): TranscriptItem[] {
  const results = new Map<string, TranscriptLine>();
  const calls = new Set<string>();
  for (const line of lines) {
    if (!line.reference) continue;
    if (isResult(line)) results.set(line.reference, line);
    else calls.add(line.reference);
  }
  const items: TranscriptItem[] = [];
  for (const line of lines) {
    if (isResult(line) && line.reference && calls.has(line.reference)) continue;
    const result = !isResult(line) && line.reference ? results.get(line.reference) : undefined;
    items.push({
      line,
      outcome: result ? (result.kind === 'fail' ? 'fail' : 'ok') : null,
      failure: result?.kind === 'fail' ? result.text : null,
    });
  }
  return items;
}
