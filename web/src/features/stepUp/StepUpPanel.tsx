import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { problemMessage } from '../../shared/api/problemMessage';
import { useNow } from '../../shared/lib/useNow';
import { Button } from '../../shared/ui/Button';
import { FormError } from '../../shared/ui/FormError';
import { sessionQuery } from '../session/queries';
import { CheckSetting } from '../dispatcher/controls';
import { stepUpSettingQuery, useRedeemStepUp, useSetStepUp } from './queries';
import './stepUp.css';

export const STEP_UP_HOW =
  'Run nullradix confirm in a terminal and open the link it prints, or choose Confirm changes in the tray menu of the desktop app. Then save again.';

const MESSAGES = {
  'step-up-required': STEP_UP_HOW,
  'command-change-needs-person': 'Only the person in the app can change this.',
  'step-up-code-invalid':
    'The confirmation code is unknown or expired. Run nullradix confirm again.',
};

function untilText(until: number): string {
  return new Date(until).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

// Confirmation state of this session and the switch that asks for it before commands change.
export function StepUpPanel() {
  const session = useQuery(sessionQuery());
  const setting = useQuery(stepUpSettingQuery());
  const set = useSetStepUp();
  const redeem = useRedeemStepUp();
  const [code, setCode] = useState('');
  const until = session.data?.elevatedUntil ?? null;
  const now = useNow(until !== null);
  const confirmed = until !== null && until > now;
  const required = setting.data?.required ?? true;

  // no form of its own: this panel sits inside the dispatcher form, and Enter must not save it
  function submit() {
    const trimmed = code.trim();
    if (trimmed === '') return;
    redeem.mutate(trimmed, {
      onSuccess: () => {
        setCode('');
      },
    });
  }

  return (
    <div className="step-up-panel">
      <CheckSetting
        label="Ask for a confirmation before changing commands"
        hint="Fields that run commands (the Claude command, its arguments, permission mode, allowed tools, Git Bash path and worktree setup) can only change after nullradix confirm. Turning this off needs the confirmation too."
        checked={required}
        disabled={setting.isPending || set.isPending}
        onChange={(checked) => {
          set.mutate(checked);
        }}
      />
      <FormError message={problemMessage(set.error, MESSAGES, 'Could not change the setting.')} />
      {confirmed ? (
        <p>Confirmed until {untilText(until)}. Command fields may change.</p>
      ) : (
        <>
          <p className="muted">
            Not confirmed. {STEP_UP_HOW.replace(' Then save again.', '')} Or enter its code here.
          </p>
          <div className="step-up-code-row">
            <input
              aria-label="Confirmation code"
              className="mono"
              value={code}
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => {
                setCode(event.target.value);
              }}
              onKeyDown={(event) => {
                if (event.key !== 'Enter') return;
                event.preventDefault();
                submit();
              }}
            />
            <Button
              type="button"
              disabled={redeem.isPending || code.trim() === ''}
              onClick={submit}
            >
              Confirm
            </Button>
          </div>
        </>
      )}
      <FormError message={problemMessage(redeem.error, MESSAGES, 'Could not confirm.')} />
    </div>
  );
}
