import { createElement, type HTMLAttributes, type ReactNode, type Ref } from 'react';

// Elements of the core around a slot that styles only the inside (Card, Panel).
export type HostTag = 'div' | 'article' | 'section' | 'form' | 'aside';

// What stays on the core element: role, ref, events, style and aria or data attributes.
export type HostAttributes = Omit<
  HTMLAttributes<HTMLElement>,
  'title' | 'className' | 'children'
> & {
  as?: HostTag;
  ref?: Ref<HTMLElement>;
};

// The core element; it carries data-slot while a theme styles the inside.
export function SlotHost({
  as = 'div',
  className,
  slot,
  replaced,
  attributes,
  children,
}: {
  as?: HostTag | undefined;
  className?: string | undefined;
  slot: string;
  replaced: boolean;
  attributes: Omit<HostAttributes, 'as'>;
  children: ReactNode;
}) {
  return createElement(
    as,
    { className, ...attributes, 'data-slot': replaced ? slot : undefined },
    children,
  );
}
