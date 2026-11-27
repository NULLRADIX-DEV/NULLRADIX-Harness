import type { CardProps } from '@nullradix/theme-sdk';
import { SlotHost, type HostAttributes } from './SlotHost';
import { useSlotted } from './useSlotted';

// A card: the element stays in the core, the slot Card of the theme may style the inside.
export function Card({
  as,
  title,
  actions,
  className,
  children,
  ...attributes
}: CardProps & HostAttributes) {
  const standard = (
    <>
      {title}
      {children}
      {actions}
    </>
  );
  const slotProps: CardProps = {
    title,
    actions,
    children,
    ...(className !== undefined && { className }),
  };
  const { node, replaced } = useSlotted('Card', slotProps, standard);
  return (
    <SlotHost as={as} className={className} slot="Card" replaced={replaced} attributes={attributes}>
      {node}
    </SlotHost>
  );
}
