import { useCallback, useState } from 'react';

function same(text: string): string {
  return text;
}

// A new saved value replaces the draft unless the user typed something else.
export function useDraft(
  value: string,
  normalize: (text: string) => string = same,
): [string, (draft: string) => void] {
  const [state, setState] = useState({ base: value, draft: value });
  let current = state;
  if (state.base !== value) {
    const typed = state.draft !== state.base && normalize(state.draft) !== normalize(value);
    current = { base: value, draft: typed ? state.draft : value };
    setState(current);
  }
  const setDraft = useCallback((draft: string) => {
    setState((previous) => ({ ...previous, draft }));
  }, []);
  return [current.draft, setDraft];
}
