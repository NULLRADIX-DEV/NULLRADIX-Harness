import { useState } from 'react';
import {
  useAddStep,
  useChangeStep,
  useDeleteTicket,
  useMoveStep,
  useMoveTicket,
  useRemoveStep,
  useUpdateTicket,
  type StepChange,
  type Ticket,
  type TicketChange,
  type TicketStatus,
} from '../tickets/queries';

export interface TicketActions {
  busy: boolean;
  // The error of the last change; a later success clears it.
  failure: unknown;
  save: (change: TicketChange) => Promise<boolean>;
  move: (status: TicketStatus, acceptance?: string) => Promise<boolean>;
  remove: () => Promise<boolean>;
  addStep: (text: string, detail?: string) => Promise<boolean>;
  changeStep: (number: number, change: StepChange) => Promise<boolean>;
  removeStep: (number: number, expectText: string) => Promise<boolean>;
  moveStep: (number: number, to: number, expectText: string) => Promise<boolean>;
}

// Every change of an open ticket, based on the version of the last read.
export function useTicketActions(ticket: Ticket): TicketActions {
  const update = useUpdateTicket();
  const moveTicket = useMoveTicket();
  const deleteTicket = useDeleteTicket();
  const add = useAddStep();
  const change = useChangeStep();
  const removeOne = useRemoveStep();
  const moveOne = useMoveStep();
  const [failure, setFailure] = useState<unknown>(null);
  const { key, version } = ticket;

  async function run(work: () => Promise<unknown>): Promise<boolean> {
    setFailure(null);
    try {
      await work();
      return true;
    } catch (error) {
      setFailure(error);
      return false;
    }
  }

  return {
    busy: [update, moveTicket, deleteTicket, add, change, removeOne, moveOne].some(
      (mutation) => mutation.isPending,
    ),
    failure,
    save: (fields) => run(() => update.mutateAsync({ key, version, change: fields })),
    move: (status, acceptance) =>
      run(() =>
        moveTicket.mutateAsync({
          key,
          version,
          status,
          ...(acceptance === undefined ? {} : { acceptance }),
        }),
      ),
    remove: () => run(() => deleteTicket.mutateAsync({ key, version })),
    addStep: (text, detail) =>
      run(() => add.mutateAsync({ key, text, ...(detail === undefined ? {} : { detail }) })),
    changeStep: (number, fields) => run(() => change.mutateAsync({ key, number, change: fields })),
    removeStep: (number, expectText) =>
      run(() => removeOne.mutateAsync({ key, number, expectText })),
    moveStep: (number, to, expectText) =>
      run(() => moveOne.mutateAsync({ key, number, to, expectText })),
  };
}
