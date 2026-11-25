import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useSearchParams } from 'react-router';
import { projectsQuery } from '../projects/queries';
import { boardsQuery } from '../boards/queries';
import { pickBoard, readBoardChoice } from '../board/boardChoice';
import '../board/board.css';
import { QuestionDetail, STATE_LABELS } from './QuestionDetail';
import { questionsQuery, type Question, type QuestionCounts, type StateFilter } from './queries';
import './questions.css';

export const QUESTION_PARAM = 'question';

const FILTERS: readonly StateFilter[] = ['open', 'answered', 'discarded', 'all'];

function count(counts: QuestionCounts | undefined, filter: StateFilter): number | null {
  if (!counts) return null;
  return filter === 'all' ? counts.open + counts.answered + counts.discarded : counts[filter];
}

function QuestionRow({
  question,
  selected,
  onOpen,
}: {
  question: Question;
  selected: boolean;
  onOpen: (key: string) => void;
}) {
  return (
    <li>
      <button
        type="button"
        className={selected ? 'qx-row on' : 'qx-row'}
        aria-current={selected ? 'true' : undefined}
        onClick={() => {
          onOpen(question.key);
        }}
      >
        <span className="card-id">{question.key}</span>
        <span className="qx-row-title">{question.title}</span>
        <span className="chips">
          {question.state !== 'open' && (
            <span className={`badge qx-state-${question.state}`}>
              {STATE_LABELS[question.state]}
            </span>
          )}
          {question.ready && <span className="badge waiting">Ready</span>}
          {question.waitingOn.length > 0 && (
            <span className="badge">
              waits on {question.waitingOn.map((w) => w.key).join(', ')}
            </span>
          )}
          {question.researched && (
            <span className="badge">
              {question.optionCount === 1 ? '1 option' : `${String(question.optionCount)} options`}
            </span>
          )}
          {question.tickets.map((ticket) => (
            <span key={ticket.key} className="tag">
              {ticket.key}
            </span>
          ))}
        </span>
      </button>
    </li>
  );
}

// Questions of one project: state filter with counts, the list, and the open question beside it.
export function QuestionsPage() {
  const projects = useQuery(projectsQuery());
  const boards = useQuery(boardsQuery());
  const [chosen, setChosen] = useState<string | null>(null);
  const [filter, setFilter] = useState<StateFilter>('open');
  const [searchParams, setSearchParams] = useSearchParams();
  const fallback = boards.data
    ? pickBoard(boards.data, readBoardChoice(window.localStorage))?.projectId
    : undefined;
  const projectId = chosen ?? fallback ?? projects.data?.[0]?.id;
  const list = useQuery({
    ...questionsQuery(projectId ?? '', filter),
    enabled: projectId !== undefined,
  });
  const openKey = searchParams.get(QUESTION_PARAM);

  function open(key: string) {
    setSearchParams({ [QUESTION_PARAM]: key });
  }

  return (
    <div className="page qx-page">
      <div className="db-board-head">
        <h1 className="page-title">Questions</h1>
        {projects.data && projectId !== undefined && (
          <label className="field db-board-picker">
            <span className="visually-hidden">Project</span>
            <select
              value={projectId}
              onChange={(event) => {
                setChosen(event.target.value);
                setSearchParams({});
              }}
            >
              {projects.data.map((project) => (
                <option key={project.id} value={project.id}>
                  {project.key} {project.name}
                </option>
              ))}
            </select>
          </label>
        )}
      </div>
      {projects.data?.length === 0 && (
        <div className="state empty">
          <h2>No project yet</h2>
          <p>Create a project under Settings first.</p>
        </div>
      )}
      {projectId !== undefined && (
        <div className="qx-layout">
          <section className="qx-list" aria-label="Question list">
            <div role="tablist" aria-label="State" className="qx-filters">
              {FILTERS.map((state) => {
                const number = count(list.data?.counts, state);
                const label = state === 'all' ? 'All' : STATE_LABELS[state];
                return (
                  <button
                    key={state}
                    type="button"
                    role="tab"
                    aria-selected={filter === state}
                    className={filter === state ? 'qx-filter on' : 'qx-filter'}
                    onClick={() => {
                      setFilter(state);
                    }}
                  >
                    {number === null ? label : `${label} (${String(number)})`}
                  </button>
                );
              })}
            </div>
            {list.status === 'pending' && <p className="muted">Loading questions...</p>}
            {list.status === 'error' && <p className="form-error">Could not load the questions.</p>}
            {list.data?.items.length === 0 && <p className="muted">No questions here.</p>}
            {list.data && list.data.items.length > 0 && (
              <ul className="qx-rows">
                {list.data.items.map((question) => (
                  <QuestionRow
                    key={question.key}
                    question={question}
                    selected={question.key === openKey}
                    onOpen={open}
                  />
                ))}
              </ul>
            )}
          </section>
          {openKey !== null && <QuestionDetail key={openKey} questionKey={openKey} />}
        </div>
      )}
    </div>
  );
}
