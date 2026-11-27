import { useEffect, useRef, useState } from 'react';
import { ApiProblemError } from '../../shared/api/errors';
import { readTabKey } from '../../shared/api/tabKey';
import {
  CONFIRM_CHANNEL,
  isConfirmMessage,
  takeConfirmCode,
  type ConfirmMessage,
} from './confirmLink';
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

// Redeems a code with this tab's key, or hands it to the other tabs; `first` was in the address.
export function StepUpCapture({ first = null }: { first?: string | null }) {
  const { mutate } = useRedeemStepUp();
  const [notice, setNotice] = useState<string | null>(null);
  const waiting = useRef<{ code: string; timer: ReturnType<typeof setTimeout> } | null>(null);

  useEffect(() => {
    const talk =
      typeof BroadcastChannel === 'undefined' ? null : new BroadcastChannel(CONFIRM_CHANNEL);
    const say = (message: ConfirmMessage) => {
      talk?.postMessage(message);
    };
    // another tab may have taken a code that was handed on, so only the own tab reports a failure
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
    const handle = (code: string) => {
      if (readTabKey() !== null) {
        confirm(code, true);
        return;
      }
      if (talk === null) {
        setNotice('Open NULLRADIX from the desktop app and enter the confirmation code there.');
        return;
      }
      setNotice('Looking for the open NULLRADIX tab...');
      const timer = setTimeout(() => {
        waiting.current = null;
        setNotice(
          'No open NULLRADIX tab took the code. Enter it in the settings of the tab that is signed in.',
        );
      }, HAND_ON_WAIT_MS);
      waiting.current = { code, timer };
      say({ type: 'code', code });
    };
    const fromAddress = () => {
      const code = takeConfirmCode();
      if (code !== null) handle(code);
    };
    const heard = (event: MessageEvent<unknown>) => {
      const message = event.data;
      if (!isConfirmMessage(message)) return;
      if (message.type === 'code' && readTabKey() !== null) {
        confirm(message.code, false, (ok) => {
          say({ type: 'done', code: message.code, ok });
        });
      }
      // a refusal is not the end: the code may be taken by another tab, or time runs out
      if (message.type === 'done' && message.ok && waiting.current?.code === message.code) {
        clearTimeout(waiting.current.timer);
        waiting.current = null;
        setNotice('Confirmed in your open NULLRADIX tab. You can close this one.');
      }
    };
    talk?.addEventListener('message', heard);
    window.addEventListener('hashchange', fromAddress);
    if (first === null) fromAddress();
    else handle(first);
    return () => {
      window.removeEventListener('hashchange', fromAddress);
      talk?.removeEventListener('message', heard);
      talk?.close();
      if (waiting.current) clearTimeout(waiting.current.timer);
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
