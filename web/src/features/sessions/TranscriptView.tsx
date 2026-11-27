import { memo, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type { TranscriptEntry } from './queries';
import type { TranscriptState } from './transcript';
import { Button } from '../../shared/ui/Button';

type Entry<K extends TranscriptEntry['kind']> = Extract<TranscriptEntry, { kind: K }>;

// Prompts longer than this show cut first.
const PROMPT_CLAMP_LINES = 12;
const PROMPT_CLAMP_CHARS = 1200;
// Distance to the end in pixels that still counts as at the bottom.
const BOTTOM_SLACK = 48;

function clock(at: string): string {
  const date = new Date(at);
  return Number.isNaN(date.getTime())
    ? ''
    : date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

// Short durations with seconds: "under 1 s", "2 s", "1 min 05 s".
export function formatSeconds(millis: number): string {
  if (millis < 1000) return 'under 1 s';
  const seconds = Math.round(millis / 1000);
  if (seconds < 60) return `${String(seconds)} s`;
  return `${String(Math.floor(seconds / 60))} min ${String(seconds % 60).padStart(2, '0')} s`;
}

// The tool's short name: `mcp__server__tool` reads `server . tool`.
export function toolName(name: string): string {
  const match = /^mcp__(.+?)__(.+)$/.exec(name);
  if (!match) return name;
  const [, rawServer = '', tool = ''] = match;
  const server = rawServer.split(/[_:]/).filter(Boolean).at(-1) ?? rawServer;
  return `${server.length > 24 ? `${server.slice(0, 8)}...` : server} \u00b7 ${tool}`;
}

// The summary without the tool name in front, since the name stands beside it.
export function toolDetail(name: string, summary: string): string {
  for (const prefix of [name, toolName(name)]) {
    if (summary.startsWith(prefix)) {
      const rest = summary.slice(prefix.length).replace(/^[:\s\u00b7-]+/, '');
      if (rest) return rest;
    }
  }
  return summary === name ? '' : summary;
}

function field(input: unknown, key: string): string | null {
  if (input === null || typeof input !== 'object' || Array.isArray(input)) return null;
  const value = (input as Record<string, unknown>)[key];
  return typeof value === 'string' ? value : null;
}

function pretty(input: unknown): string {
  if (typeof input === 'string') return input;
  return JSON.stringify(input, null, 2);
}

// Before and after of a replacement.
function Replacement({ before, after }: { before: string; after: string }) {
  return (
    <div className="tx-replace">
      <pre className="tx-code tx-before" aria-label="Before">
        {before}
      </pre>
      <pre className="tx-code tx-after" aria-label="After">
        {after}
      </pre>
    </div>
  );
}

// The input as each tool means it: a command, a replacement, a file content, else JSON.
export function ToolInput({ name, input }: { name: string; input: unknown }) {
  const path = field(input, 'file_path') ?? field(input, 'notebook_path');
  const command = field(input, 'command');
  if ((name === 'Bash' || name === 'PowerShell') && command !== null) {
    const description = field(input, 'description');
    return (
      <>
        {description && <p className="tx-note">{description}</p>}
        <pre className="tx-code" aria-label="Command">
          {command}
        </pre>
      </>
    );
  }
  const before = field(input, 'old_string');
  const after = field(input, 'new_string');
  if (name === 'Edit' && before !== null && after !== null) {
    return (
      <>
        {path && <div className="tx-path mono">{path}</div>}
        <Replacement before={before} after={after} />
      </>
    );
  }
  const content = field(input, 'content');
  if (name === 'Write' && content !== null) {
    return (
      <>
        {path && <div className="tx-path mono">{path}</div>}
        <pre className="tx-code" aria-label="Content">
          {content}
        </pre>
      </>
    );
  }
  return (
    <pre className="tx-code" aria-label="Input">
      {pretty(input)}
    </pre>
  );
}

function PromptEntry({ entry }: { entry: Entry<'prompt'> }) {
  const long =
    entry.text.split('\n').length > PROMPT_CLAMP_LINES || entry.text.length > PROMPT_CLAMP_CHARS;
  const [open, setOpen] = useState(false);
  const bodyId = useId();
  return (
    <div className="tx-prompt">
      <div className="tx-meta muted">
        <span>You</span>
        <time dateTime={entry.at}>{clock(entry.at)}</time>
      </div>
      <div id={bodyId} className={long && !open ? 'tx-bubble clamped' : 'tx-bubble'}>
        {entry.text}
      </div>
      {long && (
        <button
          type="button"
          className="btn small"
          aria-expanded={open}
          aria-controls={bodyId}
          onClick={() => {
            setOpen((value) => !value);
          }}
        >
          {open ? 'Show less' : 'Show the whole prompt'}
        </button>
      )}
    </div>
  );
}

function ThinkingEntry({ entry }: { entry: Entry<'thinking'> }) {
  const [open, setOpen] = useState(false);
  const bodyId = useId();
  const label =
    entry.durationMs == null ? 'Thought' : `Thought for ${formatSeconds(entry.durationMs)}`;
  if (entry.text.trim() === '') return <div className="tx-thinking muted">{label}</div>;
  return (
    <div className="tx-thinking">
      <button
        type="button"
        className="tx-head"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => {
          setOpen((value) => !value);
        }}
      >
        {label}
      </button>
      {open && (
        <div id={bodyId} className="tx-text muted">
          {entry.text}
        </div>
      )}
    </div>
  );
}

function ToolEntry({ entry }: { entry: Entry<'tool'> }) {
  const [open, setOpen] = useState(false);
  const bodyId = useId();
  const result = entry.result ?? null;
  const failed = result?.isError === true;
  const detail = toolDetail(entry.name, entry.summary);
  return (
    <div className={failed ? 'tx-tool failed' : 'tx-tool'}>
      <button
        type="button"
        className="tx-head"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => {
          setOpen((value) => !value);
        }}
      >
        <span className="tx-tool-name">{toolName(entry.name)}</span>
        {detail && <span className="tx-tool-summary mono">{detail}</span>}
        {failed && <span className="tx-state error">Failed</span>}
        {result === null && <span className="tx-state">No result yet</span>}
      </button>
      {open && (
        <div id={bodyId} className="tx-tool-body">
          <section aria-label="Input">
            <h4>Input</h4>
            <ToolInput name={entry.name} input={entry.input} />
          </section>
          <section aria-label="Result">
            <h4>Result</h4>
            {result === null ? (
              <p className="muted">No result yet.</p>
            ) : (
              <>
                <pre className={failed ? 'tx-code error' : 'tx-code'}>
                  {result.text.trim() === '' ? '(no output)' : result.text}
                </pre>
                {result.truncated && (
                  <p className="tx-note">Output cut to the first 20000 characters.</p>
                )}
              </>
            )}
          </section>
        </div>
      )}
    </div>
  );
}

// One entry; memo keeps known entries from rendering again when pages arrive.
const EntryView = memo(function EntryView({ entry }: { entry: TranscriptEntry }) {
  return (
    <li className={`tx-entry ${entry.kind}`} data-kind={entry.kind}>
      {entry.kind === 'prompt' && <PromptEntry entry={entry} />}
      {entry.kind === 'text' && <div className="tx-text">{entry.text}</div>}
      {entry.kind === 'thinking' && <ThinkingEntry entry={entry} />}
      {entry.kind === 'tool' && <ToolEntry entry={entry} />}
      {entry.kind === 'system' && (
        <div className={`tx-system ${entry.level}`} role="note">
          {entry.text}
        </div>
      )}
    </li>
  );
});

// The transcript as a chat that follows its end while the reader is there.
export function TranscriptView({
  transcript,
  running,
  onRetry,
}: {
  transcript: TranscriptState;
  running: boolean;
  onRetry: () => void;
}) {
  const scroll = useRef<HTMLDivElement | null>(null);
  const atBottom = useRef(true);
  const seen = useRef(0);
  const [unseen, setUnseen] = useState(0);
  const { entries } = transcript;

  function toEnd() {
    const node = scroll.current;
    if (node) node.scrollTop = node.scrollHeight;
  }

  useLayoutEffect(() => {
    const added = entries.length - seen.current;
    seen.current = entries.length;
    if (atBottom.current) toEnd();
    else if (added > 0) setUnseen((count) => count + added);
  }, [entries]);

  useEffect(() => {
    if (transcript.status === 'loading') seen.current = 0;
  }, [transcript.status]);

  function follow() {
    const node = scroll.current;
    if (!node) return;
    const bottom = node.scrollHeight - node.scrollTop - node.clientHeight <= BOTTOM_SLACK;
    atBottom.current = bottom;
    if (bottom) setUnseen(0);
  }

  if (transcript.status === 'loading') return <p className="muted">Loading the transcript...</p>;
  if (transcript.status === 'error') {
    return (
      <div className="form-error" role="alert">
        Could not load the transcript.{' '}
        <Button type="button" className="small" onClick={onRetry}>
          Try again
        </Button>
      </div>
    );
  }
  if (!transcript.available) {
    return (
      <div className="state empty">
        <h2>No transcript</h2>
        <p>The hooks named none, or it was deleted. The changes may still show.</p>
      </div>
    );
  }
  return (
    <div className="tx-wrap">
      <div
        ref={scroll}
        className="tx-scroll"
        onScroll={follow}
        tabIndex={0}
        aria-label="Transcript"
      >
        {entries.length === 0 ? (
          <p className="muted">No entries yet. They appear once the session writes.</p>
        ) : (
          <ol className="transcript">
            {entries.map((entry) => (
              <EntryView key={entry.id} entry={entry} />
            ))}
          </ol>
        )}
        {!transcript.complete && (
          <p className="muted" role="status">
            Loading more entries...
          </p>
        )}
        {transcript.skipped > 0 && (
          <p className="muted">
            {transcript.skipped === 1
              ? 'One line of the transcript could not be read.'
              : `${String(transcript.skipped)} lines of the transcript could not be read.`}
          </p>
        )}
        {running && transcript.complete && (
          <p className="tx-live muted">The session is running; new entries appear by themselves.</p>
        )}
      </div>
      {unseen > 0 && (
        <Button
          type="button"
          className="small tx-new"
          onClick={() => {
            atBottom.current = true;
            setUnseen(0);
            toEnd();
          }}
        >
          New entries
        </Button>
      )}
    </div>
  );
}
