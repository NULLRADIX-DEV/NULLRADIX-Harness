import { useQuery } from '@tanstack/react-query';
import { useId, useState, type KeyboardEvent } from 'react';
import { problemMessage, type ProblemMessages } from '../../shared/api/problemMessage';
import { useNow } from '../../shared/lib/useNow';
import { dayLabel } from '../board/TicketHistory';
import { initials } from '../board/TicketComments';
import {
  isResearching,
  questionQuery,
  questionResearchQuery,
  useAddQuestionComment,
  useAnswerQuestion,
  useDiscardQuestion,
  useReopenQuestion,
  useStartResearch,
  type QuestionDetail as Detail,
  type QuestionOption,
} from './queries';

const TIME = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' });

export const STATE_LABELS = {
  open: 'Open',
  answered: 'Answered',
  discarded: 'Discarded',
} as const;

const MESSAGES: ProblemMessages = {
  'version-conflict': 'This question changed in the meantime. It is up to date now; try again.',
  'question-state-invalid': 'This question is decided already. Reopen it first.',
  'decision-ambiguous': 'Choose an option or write an own answer, not both.',
  'decision-option-unknown': 'This option is not one of the newest round.',
  'decision-reason-missing': 'Discarding needs a reason.',
  'human-only': 'Only a person decides a question.',
  'autopilot-reason-reserved':
    'A reason starting with "Autopilot:" is kept for the dispatcher. Write it another way.',
  'comment-empty': 'A comment needs a text.',
  'question-not-found': 'This question no longer exists.',
  'question-not-open': 'Only open questions are researched.',
  'research-running': 'A research of this question is running already.',
  'board-repo-missing':
    'Link the question to a ticket of a board with a repository, or set the repository of a board of its project.',
  'dispatch-paused': 'The dispatcher is paused or no Claude account is free. Try again later.',
  'research-worktree-busy':
    'A folder of an earlier research is in the way. Check the folder next to the repository by hand.',
};

function message(error: unknown, fallback: string): string | null {
  return problemMessage(error, MESSAGES, fallback);
}

// Starts a research run by hand; the dispatcher settles it and the result arrives as live updates.
function ResearchButton({ questionKey }: { questionKey: string }) {
  const runs = useQuery(questionResearchQuery(questionKey));
  const start = useStartResearch();
  const researching = runs.data !== undefined && isResearching(runs.data);
  const error = message(start.error, 'Could not start the research.');
  return (
    <div className="qx-research">
      <button
        type="button"
        className="btn small"
        disabled={runs.data === undefined || researching || start.isPending}
        onClick={() => {
          start.mutate(questionKey);
        }}
      >
        {researching || start.isPending ? 'Researching...' : 'Research'}
      </button>
      {error !== null && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

function OptionCard({ option }: { option: QuestionOption }) {
  return (
    <>
      <span className="qx-option-head">
        <strong>{option.key}</strong> {option.title}
        {option.recommended && <span className="badge waiting">Recommended</span>}
      </span>
      <span className="qx-option-text">{option.text}</span>
      {(option.pros.length > 0 || option.cons.length > 0) && (
        <span className="qx-proscons">
          {option.pros.map((pro) => (
            <span key={`+${pro}`} className="qx-pro">
              + {pro}
            </span>
          ))}
          {option.cons.map((con) => (
            <span key={`-${con}`} className="qx-con">
              - {con}
            </span>
          ))}
        </span>
      )}
    </>
  );
}

// The decision card of an open question: an option of the newest round or an own answer.
function Decide({ detail }: { detail: Detail }) {
  const id = useId();
  const question = detail.question;
  const options = detail.research.at(-1)?.options ?? [];
  const [picked, setPicked] = useState<string | null>(null);
  const [own, setOwn] = useState('');
  const [reason, setReason] = useState('');
  const answer = useAnswerQuestion();
  const discard = useDiscardQuestion();
  const busy = answer.isPending || discard.isPending;
  const text = own.trim();
  const canAnswer = (picked !== null || text !== '') && !busy;
  const trimmed = reason.trim();
  const because = trimmed === '' ? {} : { reason: trimmed };
  const error = message(answer.error ?? discard.error, 'Could not decide the question.');

  function decide() {
    if (!canAnswer) return;
    const choice = picked === null ? { text } : { option: picked };
    answer.mutate({ key: question.key, version: question.version, ...choice, ...because });
  }

  return (
    <section className="qx-decide" aria-label="Decide">
      {options.length > 0 && (
        <div role="radiogroup" aria-label="Options" className="qx-options">
          {options.map((option) => (
            <label
              key={option.key}
              className={picked === option.key ? 'qx-option on' : 'qx-option'}
            >
              <input
                type="radio"
                name={`${id}-option`}
                checked={picked === option.key}
                onChange={() => {
                  setPicked(option.key);
                  setOwn('');
                }}
              />
              <span className="qx-option-main">
                <OptionCard option={option} />
              </span>
            </label>
          ))}
        </div>
      )}
      <label className="field">
        <span>Own answer</span>
        <input
          value={own}
          onChange={(event) => {
            setOwn(event.target.value);
            if (event.target.value.trim() !== '') setPicked(null);
          }}
        />
      </label>
      <label className="field">
        <span>Reason</span>
        <textarea
          rows={2}
          value={reason}
          onChange={(event) => {
            setReason(event.target.value);
          }}
          onKeyDown={(event: KeyboardEvent<HTMLTextAreaElement>) => {
            if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
              event.preventDefault();
              decide();
            }
          }}
        />
      </label>
      {error !== null && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      <div className="qx-actions">
        <button
          type="button"
          className="btn danger small"
          disabled={busy}
          onClick={() => {
            discard.mutate({ key: question.key, version: question.version, ...because });
          }}
        >
          Discard
        </button>
        <button type="button" className="btn primary" disabled={!canAnswer} onClick={decide}>
          {picked === null ? 'Answer' : `Answer: ${picked}`}
        </button>
      </div>
    </section>
  );
}

function Decided({ detail }: { detail: Detail }) {
  const question = detail.question;
  const decision = question.decision;
  const reopen = useReopenQuestion();
  const error = message(reopen.error, 'Could not reopen the question.');
  const chosen =
    decision?.option == null
      ? null
      : detail.research
          .flatMap((round) => round.options)
          .find((option) => option.key === decision.option);
  return (
    <section className="qx-decision" aria-label="Decision">
      {question.state === 'discarded' ? (
        <p>
          <strong>Discarded.</strong>
        </p>
      ) : (
        <p>
          <strong>Answer:</strong>{' '}
          {chosen ? `${chosen.key} ${chosen.title}` : (decision?.text ?? '')}
        </p>
      )}
      {decision?.reason != null && <p className="muted">Reason: {decision.reason}</p>}
      {decision && (
        <p className="muted">
          by {decision.byName}
          {decision.autopilot ? ' (autopilot)' : ''}
        </p>
      )}
      {error !== null && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      <button
        type="button"
        className="btn small"
        disabled={reopen.isPending}
        onClick={() => {
          reopen.mutate({ key: question.key, version: question.version });
        }}
      >
        Reopen
      </button>
    </section>
  );
}

function Comments({ detail }: { detail: Detail }) {
  const id = useId();
  const now = useNow(false);
  const [body, setBody] = useState('');
  const add = useAddQuestionComment();
  const empty = body.trim() === '';
  const error = message(add.error, 'Could not save the comment.');

  function send() {
    if (empty || add.isPending) return;
    add.mutate(
      { key: detail.question.key, body },
      {
        onSuccess: () => {
          setBody('');
        },
      },
    );
  }

  return (
    <section className="db-comments" aria-label="Comments">
      <div className="db-composer">
        <div className="db-composer-box">
          <label className="visually-hidden" htmlFor={id}>
            New comment
          </label>
          <textarea
            id={id}
            rows={2}
            placeholder="Write a comment"
            value={body}
            onChange={(event) => {
              setBody(event.target.value);
            }}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
                event.preventDefault();
                send();
              }
            }}
          />
          <div className="db-composer-actions">
            <span className="muted">Ctrl+Enter</span>
            <button
              type="button"
              className="btn primary small"
              disabled={empty || add.isPending}
              onClick={send}
            >
              Comment
            </button>
          </div>
        </div>
      </div>
      {error !== null && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      {detail.comments.length === 0 ? (
        <p className="muted">No comments yet.</p>
      ) : (
        <ol className="db-comment-list">
          {[...detail.comments].reverse().map((comment) => (
            <li key={comment.id} className="db-comment">
              <span className="db-avatar" aria-hidden="true">
                {initials(comment.authorName)}
              </span>
              <div className="db-comment-main">
                <div className="db-comment-head">
                  <strong>{comment.authorName}</strong>
                  {comment.authorKind !== 'human' && <span className="muted">agent</span>}
                  <time className="tl-time" dateTime={new Date(comment.createdAt).toISOString()}>
                    {dayLabel(comment.createdAt, now)} {TIME.format(comment.createdAt)}
                  </time>
                </div>
                <p className="db-comment-body">{comment.body}</p>
              </div>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

// One question with what it is about, its options, the decision and the comments.
export function QuestionDetail({ questionKey }: { questionKey: string }) {
  const detail = useQuery(questionQuery(questionKey));
  if (detail.status === 'pending') return <p className="muted">Loading the question...</p>;
  if (detail.status === 'error') {
    return <p className="form-error">{message(detail.error, 'Could not load the question.')}</p>;
  }
  const data = detail.data;
  const question = data.question;
  return (
    <article className="panel qx-detail" aria-label={`Question ${question.key}`}>
      <header className="qx-detail-head">
        <span className="card-id">{question.key}</span>
        <span className={`badge qx-state-${question.state}`}>{STATE_LABELS[question.state]}</span>
        {question.ready && <span className="badge waiting">Ready</span>}
        {question.state === 'open' && <ResearchButton questionKey={question.key} />}
      </header>
      <h2 className="qx-title">{question.title}</h2>
      <dl className="qx-facts">
        <dt>Asked by</dt>
        <dd>
          {question.originName}
          {question.originKind === 'human' ? '' : ' (agent)'}
        </dd>
        {question.occasion != null && (
          <>
            <dt>Occasion</dt>
            <dd>{question.occasion}</dd>
          </>
        )}
        {question.tickets.length > 0 && (
          <>
            <dt>About</dt>
            <dd>{question.tickets.map((ticket) => `${ticket.key} ${ticket.title}`).join(', ')}</dd>
          </>
        )}
        {question.waitingOn.length > 0 && (
          <>
            <dt>Waits on</dt>
            <dd>{question.waitingOn.map((w) => `${w.key} (${w.state})`).join(', ')}</dd>
          </>
        )}
        {question.tags.length > 0 && (
          <>
            <dt>Tags</dt>
            <dd>{question.tags.join(', ')}</dd>
          </>
        )}
      </dl>
      {question.description.trim() !== '' && (
        <p className="qx-description">{question.description}</p>
      )}
      {question.state === 'open' ? (
        // a comment bumps the version; the draft stays and the newest version is sent
        <Decide detail={data} />
      ) : (
        <>
          <Decided detail={data} />
          {data.research.length > 0 && (
            <ul className="qx-options readonly" aria-label="Options">
              {data.research
                .flatMap((round) => round.options)
                .map((option) => (
                  <li key={option.key} className="qx-option">
                    <span className="qx-option-main">
                      <OptionCard option={option} />
                    </span>
                  </li>
                ))}
            </ul>
          )}
        </>
      )}
      <Comments detail={data} />
    </article>
  );
}
