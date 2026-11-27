import type { ButtonProps } from '@nullradix/theme-sdk';
import { useSlotted } from './useSlotted';

const VARIANT_CLASS: Record<NonNullable<ButtonProps['variant']>, string> = {
  primary: ' primary',
  secondary: '',
  danger: ' danger',
  ghost: ' ghost',
};

// The button of the interface; the slot Button of the theme may replace it.
export function Button(props: ButtonProps) {
  const { variant = 'secondary', type = 'button', className, children, ...rest } = props;
  const standard = (
    <button
      type={type}
      className={`btn${VARIANT_CLASS[variant]}${className ? ` ${className}` : ''}`}
      {...rest}
    >
      {children}
    </button>
  );
  return useSlotted('Button', { ...props, variant, type }, standard).node;
}
