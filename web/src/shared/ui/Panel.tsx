import type { PanelProps } from '@nullradix/theme-sdk';
import { SlotHost, type HostAttributes } from './SlotHost';
import { useSlotted } from './useSlotted';

// An area: the element stays in the core, the slot Panel of the theme may style the inside.
export function Panel({
  as,
  title,
  className,
  children,
  ...attributes
}: PanelProps & HostAttributes) {
  const standard = (
    <>
      {title}
      {children}
    </>
  );
  const slotProps: PanelProps = {
    title,
    children,
    ...(className !== undefined && { className }),
  };
  const { node, replaced } = useSlotted('Panel', slotProps, standard);
  return (
    <SlotHost
      as={as}
      className={className}
      slot="Panel"
      replaced={replaced}
      attributes={attributes}
    >
      {node}
    </SlotHost>
  );
}
