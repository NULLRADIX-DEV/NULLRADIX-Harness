import type { ButtonProps, InputProps, ThemeSlots } from '@nullradix/theme-sdk';
import { screen } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { AcceptanceDialog } from '../../features/board/AcceptanceDialog';
import { TicketCard } from '../../features/board/TicketCard';
import { renderThemed } from '../../test/renderThemed';
import { ticket } from '../../test/boardServer';

beforeEach(() => {
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
});

describe('slots at the call sites', () => {
  it('styles a dialog of the app with the slots Dialog, Input and Button', async () => {
    const onConfirm = vi.fn();
    const slots: ThemeSlots = {
      Dialog: ({ children }) => <div data-testid="shell">{children}</div>,
      Input: (props: InputProps) => (
        <textarea
          aria-label={props['aria-label']}
          value={props.value}
          onChange={(event) => {
            props.onChange(event.target.value);
          }}
        />
      ),
      Button: (props: ButtonProps) => (
        <button type={props.type} disabled={props.disabled} data-themed={props.variant}>
          {props.children}
        </button>
      ),
    };
    renderThemed(
      <AcceptanceDialog ticketKey="ARC-0001" onConfirm={onConfirm} onCancel={vi.fn()} />,
      { id: 'ocean', slots },
    );
    expect(screen.getByTestId('shell')).toBeInTheDocument();
    const move = screen.getByRole('button', { name: 'Move to done' });
    expect(move).toHaveAttribute('data-themed', 'primary');
    expect(move).toBeDisabled();
    await userEvent.type(screen.getByRole('textbox', { name: 'Acceptance' }), 'It works');
    expect(move).toBeEnabled();
    await userEvent.click(move);
    expect(onConfirm).toHaveBeenCalledWith('It works');
  });

  it('styles the board card with the slots Card and Progress', () => {
    renderThemed(<TicketCard ticket={ticket({ stepsTotal: 4, stepsDone: 1 })} onOpen={vi.fn()} />, {
      id: 'ocean',
      slots: {
        Card: ({ children }) => <div data-testid="card-inside">{children}</div>,
        Progress: ({ done, total, caption }) => (
          <output>{`${String(done)} of ${String(total)} ${caption}`}</output>
        ),
      },
    });
    expect(screen.getByRole('status')).toHaveTextContent('1 of 4 steps done');
    expect(screen.queryByRole('progressbar')).not.toBeInTheDocument();
    expect(screen.getByTestId('card-inside').closest('article')).toHaveAttribute(
      'data-slot',
      'Card',
    );
  });
});
