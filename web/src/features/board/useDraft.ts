import { useCallback, useState } from 'react';

function same(text: string): string {
  return text;
}

export interface Draft {
  draft: string;
  setDraft: (draft: string) => void;
  // The user typed and the saved value changed since typing started.
  behind: boolean;
  acknowledge: () => void;
}

// A new saved value replaces the draft unless the user typed something else.
export function useDraft(value: string, normalize: (text: string) => string = same): Draft {
  const [state, setState] = useState({ base: value, from: value, draft: value });
  let current = state;
  if (state.base !== value) {
    const typed = state.draft !== state.base && normalize(state.draft) !== normalize(value);
    current = typed ? { ...state, base: value } : { base: value, from: value, draft: value };
    setState(current);
  }
  const setDraft = useCallback((draft: string) => {
    setState((previous) => ({ ...previous, draft }));
  }, []);
  const acknowledge = useCallback(() => {
    setState((previous) => ({ ...previous, from: previous.base }));
  }, []);
  const dirty = normalize(current.draft) !== normalize(current.base);
  return {
    draft: current.draft,
    setDraft,
    behind: dirty && normalize(current.base) !== normalize(current.from),
    acknowledge,
  };
}
