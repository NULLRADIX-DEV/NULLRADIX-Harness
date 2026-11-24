import type { KeyboardEvent } from 'react';
import { useDraft } from './useDraft';

function trimmed(text: string): string {
  return text.trim();
}

// Text field that saves when it is left or on Enter, and only when the value changed.
export function InlineField({
  label,
  value,
  placeholder,
  busy,
  wide = false,
  normalize = trimmed,
  onSave,
  onConflict,
}: {
  label: string;
  value: string;
  placeholder: string;
  busy: boolean;
  wide?: boolean;
  normalize?: (text: string) => string;
  onSave: (text: string) => void;
  onConflict: (label: string) => void;
}) {
  const { draft, setDraft, behind, acknowledge } = useDraft(value, normalize);

  function commit() {
    if (busy || normalize(draft) === normalize(value)) return;
    if (behind) {
      acknowledge();
      onConflict(label);
      return;
    }
    onSave(draft);
  }

  function onKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key !== 'Enter') return;
    event.preventDefault();
    commit();
  }

  return (
    <label className={wide ? 'field wide' : 'field'}>
      <span>{label}</span>
      <input
        value={draft}
        placeholder={placeholder}
        autoComplete="off"
        onChange={(event) => {
          setDraft(event.target.value);
        }}
        onBlur={commit}
        onKeyDown={onKeyDown}
      />
    </label>
  );
}
