import { useEffect, useRef, type KeyboardEvent, type ReactNode, type SubmitEvent } from 'react';
import { createPortal } from 'react-dom';

const FOCUSABLE = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

function focusables(node: HTMLElement): HTMLElement[] {
  return Array.from(node.querySelectorAll<HTMLElement>(FOCUSABLE));
}

export interface ModalProps {
  labelledBy: string;
  onEscape: () => void;
  onSubmit: (event: SubmitEvent<HTMLFormElement>) => void;
  className?: string;
  children: ReactNode;
}

// Modal form over the page: focus moves in on open, stays inside on Tab and returns on close.
export function Modal({
  labelledBy,
  onEscape,
  onSubmit,
  className = 'panel db-modal',
  children,
}: ModalProps) {
  const ref = useRef<HTMLFormElement>(null);

  useEffect(() => {
    const node = ref.current;
    const previous = document.activeElement;
    if (node) (focusables(node)[0] ?? node).focus();
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, []);

  function onKeyDown(event: KeyboardEvent<HTMLFormElement>) {
    const node = ref.current;
    if (!node) return;
    if (event.key === 'Escape') {
      // the ticket panel below closes on Escape too
      event.stopPropagation();
      onEscape();
      return;
    }
    if (event.key !== 'Tab') return;
    const items = focusables(node);
    const first = items[0];
    const last = items.at(-1);
    if (!first || !last) return;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  return createPortal(
    <div className="db-modal-backdrop">
      <form
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelledBy}
        tabIndex={-1}
        className={className}
        onKeyDown={onKeyDown}
        onSubmit={onSubmit}
      >
        {children}
      </form>
    </div>,
    document.body,
  );
}
