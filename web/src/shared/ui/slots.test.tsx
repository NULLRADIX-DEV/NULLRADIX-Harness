import type {
  ButtonProps,
  CardProps,
  DialogProps,
  InputProps,
  PanelProps,
  ProgressProps,
  StartAnimationProps,
  ThemeDefinition,
  ThemeSlots,
} from '@nullradix/theme-sdk';
import { act, render, screen, waitFor } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, describe, expect, expectTypeOf, it, vi } from 'vitest';
import { Modal } from '../../features/board/Modal';
import { mockMatchMedia } from '../../test/matchMedia';
import { renderThemed } from '../../test/renderThemed';
import { START_OVERLAY_ID } from '../lib/startOverlay';
import { getThemeNotice } from '../lib/themeNotice';
import { Button } from './Button';
import { Card } from './Card';
import { Input } from './Input';
import { Panel } from './Panel';
import { Progress } from './Progress';
import { START_ANIMATION_MAX_MS, StartAnimation } from './StartAnimation';
import { ThemeIcon } from './ThemeIcon';
import { useTheme } from './ThemeProvider';
import { useMermaidLook } from './useMermaidLook';
import { useSlot } from './useSlot';
import type { SlotProps } from './useSlotted';

const OCEAN = 'ocean';

function theme(slots: ThemeSlots, extra: Partial<ThemeDefinition> = {}): ThemeDefinition {
  return { id: OCEAN, slots, ...extra };
}

function Broken(): never {
  throw new Error('broken theme component');
}

beforeEach(() => {
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
});

describe('slot props match the SDK', () => {
  it('types every component slot by its SDK props', () => {
    expectTypeOf<SlotProps<'Button'>>().toEqualTypeOf<ButtonProps>();
    expectTypeOf<SlotProps<'Card'>>().toEqualTypeOf<CardProps>();
    expectTypeOf<SlotProps<'Panel'>>().toEqualTypeOf<PanelProps>();
    expectTypeOf<SlotProps<'Dialog'>>().toEqualTypeOf<DialogProps>();
    expectTypeOf<SlotProps<'Input'>>().toEqualTypeOf<InputProps>();
    expectTypeOf<SlotProps<'Progress'>>().toEqualTypeOf<ProgressProps>();
    expectTypeOf<SlotProps<'StartAnimation'>>().toEqualTypeOf<StartAnimationProps>();
  });

  it('gives a component without a theme the built-in look', () => {
    function Probe() {
      return <span>{useSlot('Button') ? 'themed' : 'built-in'}</span>;
    }
    render(<Probe />);
    expect(screen.getByText('built-in')).toBeInTheDocument();
  });
});

describe('Button', () => {
  it('renders the theme button with exactly the SDK props', () => {
    const received = vi.fn();
    const Themed = (props: ButtonProps) => {
      received(props);
      return <button type="button">themed {props.children}</button>;
    };
    renderThemed(
      <Button variant="primary" className="small" aria-label="Save it">
        Save
      </Button>,
      theme({ Button: Themed }),
    );
    expect(screen.getByRole('button', { name: 'themed Save' })).toBeInTheDocument();
    expect(received.mock.calls[0]?.[0]).toEqual({
      variant: 'primary',
      type: 'button',
      className: 'small',
      'aria-label': 'Save it',
      children: 'Save',
    });
  });

  it('renders the built-in button when the theme registers no Button', () => {
    renderThemed(
      <Button variant="danger" className="small">
        Delete
      </Button>,
      theme({}),
    );
    expect(screen.getByRole('button', { name: 'Delete' })).toHaveClass('btn', 'danger', 'small');
  });

  it('falls back to the built-in button when the theme component throws, and says so once', () => {
    renderThemed(
      <>
        <Button>One</Button>
        <Button>Two</Button>
      </>,
      theme({ Button: Broken }),
    );
    expect(screen.getByRole('button', { name: 'One' })).toHaveClass('btn');
    expect(screen.getByRole('button', { name: 'Two' })).toHaveClass('btn');
    expect(getThemeNotice()?.message).toBe(
      'The theme "ocean" has a component that failed - the default is shown.',
    );
  });
});

describe('Input', () => {
  it('hands the typed text to onChange through the theme field', async () => {
    const Themed = (props: InputProps) => (
      <input
        data-themed={props.multiline ? 'area' : 'line'}
        aria-label={props['aria-label']}
        value={props.value}
        onChange={(event) => {
          props.onChange(event.target.value);
        }}
      />
    );
    function Field() {
      const [text, setText] = useState('');
      return <Input aria-label="Name" value={text} onChange={setText} />;
    }
    renderThemed(<Field />, theme({ Input: Themed }));
    const field = screen.getByRole('textbox', { name: 'Name' });
    expect(field).toHaveAttribute('data-themed', 'line');
    await userEvent.type(field, 'abc');
    expect(field).toHaveValue('abc');
  });

  it('keeps the built-in textarea for multiline without a theme field', async () => {
    const onChange = vi.fn();
    renderThemed(
      <Input multiline rows={3} aria-label="Notes" value="" onChange={onChange} />,
      theme({}),
    );
    await userEvent.type(screen.getByRole('textbox', { name: 'Notes' }), 'x');
    expect(onChange).toHaveBeenCalledWith('x');
    expect(screen.getByRole('textbox', { name: 'Notes' }).tagName).toBe('TEXTAREA');
  });

  it('falls back to the built-in field when the theme field throws', () => {
    renderThemed(
      <Input aria-label="Name" value="kept" onChange={vi.fn()} />,
      theme({ Input: Broken }),
    );
    expect(screen.getByRole('textbox', { name: 'Name' })).toHaveValue('kept');
  });
});

describe('Card and Panel', () => {
  it('keep the element in the core and pass only the SDK props to the theme', () => {
    const cardProps = vi.fn();
    const panelProps = vi.fn();
    renderThemed(
      <>
        <Card as="article" className="card" aria-label="A card" title="Head" actions="Act">
          inside
        </Card>
        <Panel as="section" className="panel" aria-label="An area" title="Area">
          inside
        </Panel>
      </>,
      theme({
        Card: (props: CardProps) => {
          cardProps(props);
          return <div data-testid="card-inside">{props.children}</div>;
        },
        Panel: (props: PanelProps) => {
          panelProps(props);
          return <div data-testid="panel-inside">{props.children}</div>;
        },
      }),
    );
    const card = screen.getByRole('article', { name: 'A card' });
    expect(card).toHaveAttribute('data-slot', 'Card');
    expect(card).toHaveClass('card');
    expect(card).toContainElement(screen.getByTestId('card-inside'));
    const panel = screen.getByRole('region', { name: 'An area' });
    expect(panel).toHaveAttribute('data-slot', 'Panel');
    expect(panel).toContainElement(screen.getByTestId('panel-inside'));
    expect(cardProps.mock.calls[0]?.[0]).toEqual({
      title: 'Head',
      actions: 'Act',
      className: 'card',
      children: 'inside',
    });
    expect(panelProps.mock.calls[0]?.[0]).toEqual({
      title: 'Area',
      className: 'panel',
      children: 'inside',
    });
  });

  it('show the built-in inside without data-slot when no theme replaces them', () => {
    renderThemed(
      <>
        <Card as="article" className="card" aria-label="A card" title="Head">
          inside
        </Card>
        <Panel as="section" aria-label="An area">
          inside
        </Panel>
      </>,
      theme({}),
    );
    expect(screen.getByRole('article', { name: 'A card' })).not.toHaveAttribute('data-slot');
    expect(screen.getByRole('article', { name: 'A card' })).toHaveTextContent('Headinside');
    expect(screen.getByRole('region', { name: 'An area' })).not.toHaveAttribute('data-slot');
  });

  it('fall back to the built-in inside when the theme component throws', () => {
    renderThemed(
      <>
        <Card as="article" aria-label="A card">
          card inside
        </Card>
        <Panel as="section" aria-label="An area">
          panel inside
        </Panel>
      </>,
      theme({ Card: Broken, Panel: Broken }),
    );
    expect(screen.getByRole('article', { name: 'A card' })).toHaveTextContent('card inside');
    expect(screen.getByRole('region', { name: 'An area' })).toHaveTextContent('panel inside');
  });
});

describe('Progress', () => {
  it('passes done, total, label and caption to the theme and drops the built-in look', () => {
    const received = vi.fn();
    renderThemed(
      <Progress done={2} total={5} label="Steps done" caption="steps done">
        <span>built-in bar</span>
      </Progress>,
      theme({
        Progress: (props: ProgressProps) => {
          received(props);
          return <output>{`${String(props.done)}/${String(props.total)} ${props.caption}`}</output>;
        },
      }),
    );
    expect(screen.getByRole('status')).toHaveTextContent('2/5 steps done');
    expect(screen.queryByText('built-in bar')).not.toBeInTheDocument();
    expect(received.mock.calls[0]?.[0]).toEqual({
      done: 2,
      total: 5,
      label: 'Steps done',
      caption: 'steps done',
    });
  });

  it('shows the built-in look when the theme component throws', () => {
    renderThemed(
      <Progress done={1} total={2} label="Steps done" caption="steps done">
        <span>built-in bar</span>
      </Progress>,
      theme({ Progress: Broken }),
    );
    expect(screen.getByText('built-in bar')).toBeInTheDocument();
  });
});

describe('Dialog', () => {
  function open(slots: ThemeSlots, onEscape = vi.fn()) {
    return renderThemed(
      <Modal
        labelledBy="title"
        role="alertdialog"
        className="panel db-modal"
        onEscape={onEscape}
        onSubmit={vi.fn()}
      >
        <strong id="title">Sure?</strong>
        <button type="button">Yes</button>
      </Modal>,
      theme(slots),
    );
  }

  it('lets the theme style the shell while role, focus and Escape stay in the core', async () => {
    const received = vi.fn();
    const onEscape = vi.fn();
    open(
      {
        Dialog: (props: DialogProps) => {
          received(props);
          return <div data-testid="shell">{props.children}</div>;
        },
      },
      onEscape,
    );
    const dialog = screen.getByRole('alertdialog', { name: 'Sure?' });
    expect(dialog).toHaveAttribute('data-slot', 'Dialog');
    expect(dialog).toContainElement(screen.getByTestId('shell'));
    expect(screen.getByRole('button', { name: 'Yes' })).toHaveFocus();
    await userEvent.keyboard('{Escape}');
    expect(onEscape).toHaveBeenCalledTimes(1);
    expect(received.mock.calls[0]?.[0]).toMatchObject({
      labelledBy: 'title',
      role: 'alertdialog',
      className: 'panel db-modal',
    });
    expect(Object.keys(received.mock.calls[0]?.[0] as object).sort()).toEqual([
      'children',
      'className',
      'labelledBy',
      'role',
    ]);
  });

  it('shows the built-in dialog when the theme shell throws', () => {
    open({ Dialog: Broken });
    expect(screen.getByRole('alertdialog', { name: 'Sure?' })).toContainElement(
      screen.getByRole('button', { name: 'Yes' }),
    );
  });
});

describe('Icon', () => {
  it('replaces a core icon by name and falls back when the theme icon throws', () => {
    const { unmount } = renderThemed(
      <ThemeIcon name="folder">
        <svg data-testid="built-in" />
      </ThemeIcon>,
      theme({ Icon: ({ name }) => <i data-testid={`icon-${name}`} /> }),
    );
    expect(screen.getByTestId('icon-folder')).toBeInTheDocument();
    expect(screen.queryByTestId('built-in')).not.toBeInTheDocument();
    unmount();
    renderThemed(
      <ThemeIcon name="folder">
        <svg data-testid="built-in" />
      </ThemeIcon>,
      theme({ Icon: Broken }),
    );
    expect(screen.getByTestId('built-in')).toBeInTheDocument();
  });
});

describe('Mermaid look', () => {
  function Look() {
    const look = useMermaidLook();
    return <span>{look()?.theme ?? 'built-in'}</span>;
  }

  it('reads the look of the theme on every call', () => {
    const mermaid = vi.fn(() => ({ theme: 'forest' as const }));
    renderThemed(<Look />, theme({}, { mermaid }));
    expect(screen.getByText('forest')).toBeInTheDocument();
    expect(mermaid).toHaveBeenCalled();
  });

  it('keeps the built-in look and says so once when the theme look throws', () => {
    renderThemed(<Look />, theme({}, { mermaid: Broken }));
    expect(screen.getByText('built-in')).toBeInTheDocument();
    expect(getThemeNotice()?.message).toContain('"ocean"');
  });
});

describe('StartAnimation', () => {
  beforeEach(() => {
    const overlay = document.createElement('div');
    overlay.id = START_OVERLAY_ID;
    document.body.append(overlay);
  });

  afterEach(() => {
    document.getElementById(START_OVERLAY_ID)?.remove();
    vi.useRealTimers();
  });

  function Themed({ onDone }: StartAnimationProps) {
    return (
      <button type="button" onClick={onDone}>
        skip intro
      </button>
    );
  }

  it('covers the app instead of the built-in overlay until onDone', async () => {
    renderThemed(<StartAnimation />, theme({ StartAnimation: Themed }));
    expect(screen.getByText('skip intro')).toBeInTheDocument();
    expect(document.getElementById(START_OVERLAY_ID)).toBeNull();
    await userEvent.click(screen.getByText('skip intro'));
    expect(screen.queryByText('skip intro')).not.toBeInTheDocument();
  });

  it('ends after the limit when onDone never comes', () => {
    vi.useFakeTimers();
    renderThemed(<StartAnimation />, theme({ StartAnimation: () => <span>intro</span> }));
    expect(screen.getByText('intro')).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(START_ANIMATION_MAX_MS - 1);
    });
    expect(screen.getByText('intro')).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(screen.queryByText('intro')).not.toBeInTheDocument();
  });

  it('leaves the built-in overlay and shows nothing when the theme animation throws', () => {
    renderThemed(<StartAnimation />, theme({ StartAnimation: Broken }));
    expect(document.getElementById(START_OVERLAY_ID)).not.toBeNull();
    expect(document.querySelector('.theme-start-anim')).toBeNull();
    expect(getThemeNotice()?.kind).toBe('error');
  });

  it('shows no animation for reduced motion', () => {
    mockMatchMedia(true);
    renderThemed(<StartAnimation />, theme({ StartAnimation: Themed }));
    expect(screen.queryByText('skip intro')).not.toBeInTheDocument();
    expect(document.getElementById(START_OVERLAY_ID)).not.toBeNull();
  });

  it('shows no animation without a theme slot', () => {
    renderThemed(<StartAnimation />, theme({}));
    expect(document.querySelector('.theme-start-anim')).toBeNull();
    expect(document.getElementById(START_OVERLAY_ID)).not.toBeNull();
  });
});

describe('only the active theme counts', () => {
  function Themed({ children }: ButtonProps) {
    return <button type="button">themed {children}</button>;
  }

  function Switcher() {
    const { setMode } = useTheme();
    return (
      <>
        <button
          type="button"
          onClick={() => {
            setMode('light');
          }}
        >
          use light
        </button>
        <button
          type="button"
          onClick={() => {
            setMode(OCEAN);
          }}
        >
          use ocean
        </button>
        <Button>Save</Button>
      </>
    );
  }

  it('drops the slots of a theme that is not shown any more, and brings them back', async () => {
    renderThemed(<Switcher />, theme({ Button: Themed }));
    expect(screen.getByRole('button', { name: 'themed Save' })).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'use light' }));
    expect(screen.getByRole('button', { name: 'Save' })).toHaveClass('btn');
    await userEvent.click(screen.getByRole('button', { name: 'use ocean' }));
    expect(screen.getByRole('button', { name: 'themed Save' })).toBeInTheDocument();
  });

  it('drops the slots without a reload once the code switch is turned off', async () => {
    const view = renderThemed(<Switcher />, theme({ Button: Themed }));
    expect(screen.getByRole('button', { name: 'themed Save' })).toBeInTheDocument();
    view.state.list = {
      ...view.state.list,
      items: view.state.list.items.map((item) => ({ ...item, codeEnabled: false })),
    };
    await act(() => view.queryClient.invalidateQueries());
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Save' })).toHaveClass('btn');
    });
  });
});
