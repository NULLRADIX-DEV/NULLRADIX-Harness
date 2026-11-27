import type { InputProps } from '@nullradix/theme-sdk';
import { useSlotted } from './useSlotted';

// A text field (textarea if multiline) giving its text to onChange; the slot Input may replace it.
export function Input(props: InputProps) {
  const { onChange, multiline, rows, type, ...rest } = props;
  const standard = multiline ? (
    <textarea
      rows={rows}
      {...rest}
      onChange={(event) => {
        onChange(event.target.value);
      }}
    />
  ) : (
    <input
      type={type}
      {...rest}
      onChange={(event) => {
        onChange(event.target.value);
      }}
    />
  );
  return useSlotted('Input', props, standard).node;
}
