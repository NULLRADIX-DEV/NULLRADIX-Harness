import {
  formatCost,
  formatTokens,
  runDuration,
  runModel,
  runOutcome,
  runTitle,
  totalTokens,
} from './format';
import type { Run } from './queries';

// Badge, title and muted facts of a run; the content of its disclosure button.
export function RunHead({ run, now }: { run: Run; now: number }) {
  const outcome = runOutcome(run);
  const usage = run.result?.usage;
  const tokens = totalTokens(usage);
  const facts = [
    runDuration(run, now),
    runModel(run),
    usage?.costUsd != null ? formatCost(usage.costUsd) : null,
    tokens !== null ? `${formatTokens(tokens)} tokens` : null,
  ].filter((fact): fact is string => fact !== null);
  return (
    <>
      <span className={`run-status run-tone-${outcome.tone}`}>{outcome.label}</span>
      <span className="run-title">{runTitle(run)}</span>
      {facts.length > 0 && <span className="muted run-facts">{facts.join(' \u00b7 ')}</span>}
    </>
  );
}
