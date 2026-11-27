import { useState } from 'react';
import { screenshotUrl, type Screenshot } from './dispatchQueries';
import { Button } from '../../shared/ui/Button';

// Thumbnails of the screenshots a run took; one opens larger below them.
export function RunShots({ runId, shots }: { runId: string; shots: readonly Screenshot[] }) {
  const [openId, setOpenId] = useState<string | null>(null);
  const open = shots.find((shot) => shot.id === openId);
  if (shots.length === 0) return null;
  return (
    <section className="run-shots" aria-label="Screenshots">
      <h4>Screenshots</h4>
      <ul className="run-thumbs">
        {shots.map((shot, index) => (
          <li key={shot.id}>
            <button
              type="button"
              className="run-thumb"
              aria-label={`Screenshot ${String(index + 1)} of ${String(shots.length)}`}
              aria-pressed={shot.id === openId}
              onClick={() => {
                setOpenId(shot.id === openId ? null : shot.id);
              }}
            >
              <img src={screenshotUrl(runId, shot.id)} alt="" loading="lazy" />
            </button>
          </li>
        ))}
      </ul>
      {open && (
        <figure className="run-shot-large">
          <img
            src={screenshotUrl(runId, open.id)}
            alt={`Screenshot ${String(shots.indexOf(open) + 1)}`}
          />
          <Button
            type="button"
            className="small"
            onClick={() => {
              setOpenId(null);
            }}
          >
            Close screenshot
          </Button>
        </figure>
      )}
    </section>
  );
}
