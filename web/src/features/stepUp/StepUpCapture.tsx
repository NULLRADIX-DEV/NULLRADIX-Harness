import { useEffect, useRef, useState } from 'react';
import { ApiProblemError } from '../../shared/api/errors';
import { hasTabChannel, listenTab, postTab } from '../../shared/api/tabChannel';
import { readTabKey } from '../../shared/api/tabKey';
import { takeConfirmCode } from './confirmLink';
import { useRedeemStepUp } from './queries';
import './stepUp.css';

// How long a tab without a key waits for another tab to take the code.
export const HAND_ON_WAIT_MS = 3000;

function failure(error: unknown): string {
  if (error instanceof ApiProblemError && error.code === 'step-up-code-invalid') {
    return 'The confirmation code is unknown or expired. Run nullradix confirm again.';
  }
  return 'Could not confirm. Open NULLRADIX from the desktop app or nullradix open first.';
}

// Redeems a code with the key of this tab or hands it to the other tabs.
export function StepUpCapture({ first = null }: { first?: string | null }) {
  const { mutate } = useRedeemStepUp();
  const [notice, setNotice] = useState<string | null>(null);
  const waiting = useRef<{ code: string; timer?: ReturnType<typeof setTimeout> } | null>(null);
  // a code works once, and StrictMode runs the effect twice
  const handled = useRef(new Set<string>());

  useEffect(() => {
    // only the own tab reports a failure: another tab may have taken the code first
    const confirm = (code: string, own: boolean, onDone?: (ok: boolean) => void) => {
      mutate(code, {
        onSuccess: () => {
          setNotice('Confirmed for 10 minutes. You can change the command fields now.');
          onDone?.(true);
        },
        onError: (error) => {
          if (own) setNotice(failure(error));
          onDone?.(false);
        },
      });
    };
    // armed again by the second run of the effect: the first run cleared it
    const giveUpLater = () => {
      const entry = waiting.current;
      if (entry === null) return;
      entry.timer = setTimeout(() => {
        waiting.current = null;
        setNotice(
          'No open NULLRADIX tab took the code. Enter it in the settings of the tab that is signed in.',
        );
      }, HAND_ON_WAIT_MS);
    };
    const handle = (code: string) => {
      if (handled.current.has(code)) return;
      handled.current.add(code);
      if (readTabKey() !== null) {
        confirm(code, true);
        return;
      }
      if (!hasTabChannel()) {
        setNotice('Open NULLRADIX from the desktop app and enter the confirmation code there.');
        return;
      }
      setNotice('Looking for the open NULLRADIX tab...');
      waiting.current = { code };
      giveUpLater();
      postTab({ type: 'code', code });
    };
    const fromAddress = () => {
      const code = takeConfirmCode();
      if (code !== null) handle(code);
    };
    const stop = listenTab((message) => {
      if (message.type === 'code' && readTabKey() !== null && !handled.current.has(message.code)) {
        handled.current.add(message.code);
        confirm(message.code, false, (ok) => {
          postTab({ type: 'done', code: message.code, ok });
        });
      }
      // a refusal is not the end: the code may be taken by another tab, or time runs out
      if (message.type === 'done' && message.ok && waiting.current?.code === message.code) {
        clearTimeout(waiting.current.timer);
        waiting.current = null;
        setNotice('Confirmed in your open NULLRADIX tab. You can close this one.');
      }
    });
    window.addEventListener('hashchange', fromAddress);
    giveUpLater();
    if (first === null) fromAddress();
    else handle(first);
    return () => {
      window.removeEventListener('hashchange', fromAddress);
      stop();
      clearTimeout(waiting.current?.timer);
    };
  }, [mutate, first]);

  if (notice === null) return null;
  return (
    <div className="step-up-notice panel" role="status">
      <span>{notice}</span>
      <button
        type="button"
        className="btn"
        onClick={() => {
          setNotice(null);
        }}
      >
        Dismiss
      </button>
    </div>
  );
}
