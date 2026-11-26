import { useState } from 'react';
import { addDays, formatDay } from './format';
import type { DispatchStats } from './queries';

// Up to this many buckets every one has a date label, beyond it every fourth.
const LABEL_ALL = 14;

// Axis top: a multiple of 4, at least 4, so the middle stays whole.
export function axisMax(max: number): number {
  return Math.ceil(Math.max(4, max) / 4) * 4;
}

function bucketTotal(point: { done: number; aborted: number; other: number }): number {
  return point.done + point.aborted + point.other;
}

// Stacked columns per day or week; numbers show on hover and in the text alternative.
export function RunsChart({ series }: { series: DispatchStats['series'] }) {
  const [hover, setHover] = useState(-1);
  const { points, unit } = series;
  const top = axisMax(Math.max(0, ...points.map(bucketTotal)));
  const pct = (n: number) => `${String((n / top) * 100)}%`;
  const when = (start: string) =>
    unit === 'day' ? formatDay(start) : `${formatDay(start)} - ${formatDay(addDays(start, 6))}`;
  const total = points.reduce((sum, point) => sum + bucketTotal(point), 0);
  const noun = unit === 'day' ? 'day' : 'week';
  return (
    <figure className="runs-chart">
      <figcaption className="visually-hidden">
        {`Runs per ${noun}: ${String(total)} in ${String(points.length)} ${noun}${points.length === 1 ? '' : 's'}`}
      </figcaption>
      <div className="runs-chart-body" role="img" aria-label={`Runs per ${noun}, table below`}>
        <div className="runs-axis mono" aria-hidden="true">
          <span>{top}</span>
          <span>{top / 2}</span>
          <span>0</span>
        </div>
        <div className="runs-plot-wrap">
          <div
            className="runs-plot"
            onMouseLeave={() => {
              setHover(-1);
            }}
          >
            {points.map((point, index) => (
              <div
                key={point.start}
                className={`runs-slot${hover === index ? ' hover' : ''}`}
                data-start={point.start}
                onMouseEnter={() => {
                  setHover(index);
                }}
                title={`${when(point.start)}: ${String(point.done)} done, ${String(point.aborted)} aborted, ${String(point.other)} question / blocked`}
              >
                <div className="runs-seg done" style={{ height: pct(point.done) }} />
                <div className="runs-seg fail" style={{ height: pct(point.aborted) }} />
                <div className="runs-seg other" style={{ height: pct(point.other) }} />
                {hover === index && (
                  <div className="runs-tip" aria-hidden="true">
                    <strong>{when(point.start)}</strong>
                    <span className="mono done">{point.done} done</span>
                    <span className="mono fail">{point.aborted} aborted</span>
                    <span className="mono other">{point.other} question / blocked</span>
                  </div>
                )}
              </div>
            ))}
          </div>
          <div className="runs-labels mono" aria-hidden="true">
            {points.map((point, index) => (
              <span key={point.start}>
                {unit === 'week' || points.length <= LABEL_ALL || index % 4 === 0
                  ? formatDay(point.start)
                  : ''}
              </span>
            ))}
          </div>
        </div>
      </div>
      <table className="visually-hidden">
        <caption>{`Runs per ${noun}`}</caption>
        <thead>
          <tr>
            <th scope="col">{unit === 'day' ? 'Day' : 'Week'}</th>
            <th scope="col">Done</th>
            <th scope="col">Aborted</th>
            <th scope="col">Question / blocked</th>
          </tr>
        </thead>
        <tbody>
          {points.map((point) => (
            <tr key={point.start}>
              <th scope="row">{when(point.start)}</th>
              <td>{point.done}</td>
              <td>{point.aborted}</td>
              <td>{point.other}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </figure>
  );
}
