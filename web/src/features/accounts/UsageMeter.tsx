import { useQuery } from '@tanstack/react-query';
import { Progress } from '../../shared/ui/Progress';
import { usageQuery } from './queries';
import './usageMeter.css';

// The five hour limit of the first account, for a theme shell.
export function UsageMeter() {
  const statuses = useQuery(usageQuery());
  const usage = statuses.data?.[0]?.usage;
  if (!usage) return null;
  if (!usage.available || !usage.fiveHour) {
    return <span className="usage-meter">Claude usage unavailable</span>;
  }
  const { percent } = usage.fiveHour;
  return (
    <Progress
      done={percent}
      total={100}
      label="Claude usage in the 5 hour window"
      caption="of the 5 hour limit"
    >
      <span className="usage-meter">
        <progress value={percent} max={100} aria-label="Claude usage in the 5 hour window" />
        <span>{percent}% of 5 h</span>
      </span>
    </Progress>
  );
}
