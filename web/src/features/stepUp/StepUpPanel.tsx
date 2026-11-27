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

const TURN_OFF_HOW =
  'To turn the check off, run nullradix confirm --no-open in a terminal and enter its code here. A confirmed session is not enough.';

const CODE_INVALID = 'The confirmation code is unknown or expired. Run nullradix confirm again.';

const MESSAGES = {
  'step-up-required': STEP_UP_HOW,
  'step-up-code-required': TURN_OFF_HOW,
  'command-change-needs-person': 'Only the person in the app can change this.',
  'step-up-code-invalid': CODE_INVALID,
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
  const [turningOff, setTurningOff] = useState(false);
  const until = session.data?.elevatedUntil ?? null;
  const now = useNow(until !== null);
  const confirmed = until !== null && until > now;
  const required = setting.data?.required ?? true;
  const asking = turningOff || !confirmed;

  // no form of its own: this panel sits inside the dispatcher form, and Enter must not save it
  function submit() {
    const trimmed = code.trim();
    if (trimmed === '') return;
    if (turningOff) {
      set.mutate(
        { required: false, code: trimmed },
        {
          onSuccess: () => {
            setCode('');
            setTurningOff(false);
          },
        },
      );
      return;
    }
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
        hint="Fields that run commands (the Claude command, its arguments, permission mode, allowed tools, Git Bash path, worktree setup and the folder of an account outside the accounts folder) can only change after nullradix confirm."
        checked={required}
        disabled={setting.isPending || set.isPending}
        onChange={(checked) => {
          set.reset();
          if (checked) {
            setTurningOff(false);
            set.mutate({ required: true });
          } else {
            setTurningOff(true);
          }
        }}
      />
      {turningOff && <p className="muted">{TURN_OFF_HOW}</p>}
      <FormError message={problemMessage(set.error, MESSAGES, 'Could not change the setting.')} />
      {confirmed && !turningOff && (
        <p>Confirmed until {untilText(until)}. Command fields may change.</p>
      )}
      {asking && (
        <>
          {!turningOff && (
            <p className="muted">
              Not confirmed. {STEP_UP_HOW.replace(' Then save again.', '')} Or enter its code here.
            </p>
          )}
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
              disabled={redeem.isPending || set.isPending || code.trim() === ''}
              onClick={submit}
            >
              {turningOff ? 'Turn off' : 'Confirm'}
            </Button>
            {turningOff && (
              <Button
                type="button"
                onClick={() => {
                  setTurningOff(false);
                  setCode('');
                  set.reset();
                }}
              >
                Cancel
              </Button>
            )}
          </div>
        </>
      )}
      <FormError message={problemMessage(redeem.error, MESSAGES, 'Could not confirm.')} />
    </div>
  );
}
