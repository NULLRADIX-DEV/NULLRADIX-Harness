import { render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from 'vitest';
import { forgetThemeFailures } from '../lib/themeFailure';
import { getThemeNotice, setThemeNotice } from '../lib/themeNotice';
import { SlotBoundary } from './SlotBoundary';

function Broken(): never {
  throw new Error('broken theme');
}

let consoleError: MockInstance<typeof console.error>;

beforeEach(() => {
  consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined);
});

afterEach(() => {
  forgetThemeFailures();
  setThemeNotice(undefined);
});

describe('SlotBoundary', () => {
  it('shows the fallback and logs when the theme component throws', () => {
    render(
      <SlotBoundary fallback={<span>built-in</span>} themeId="ocean" where="the slot Button">
        <Broken />
      </SlotBoundary>,
    );
    expect(screen.getByText('built-in')).toBeInTheDocument();
    expect(
      consoleError.mock.calls.some((call) =>
        String(call[0]).includes('Theme "ocean": the slot Button failed'),
      ),
    ).toBe(true);
  });

  it('shows the children while nothing throws', () => {
    render(
      <SlotBoundary fallback={<span>built-in</span>} themeId="ocean" where="the slot Button">
        <span>themed</span>
      </SlotBoundary>,
    );
    expect(screen.getByText('themed')).toBeInTheDocument();
    expect(screen.queryByText('built-in')).not.toBeInTheDocument();
    expect(getThemeNotice()).toBeUndefined();
  });

  it('tells the person once per theme, however many components fail', () => {
    render(
      <>
        <SlotBoundary fallback={<span>one</span>} themeId="ocean" where="the slot Button">
          <Broken />
        </SlotBoundary>
        <SlotBoundary fallback={<span>two</span>} themeId="ocean" where="the slot Card">
          <Broken />
        </SlotBoundary>
      </>,
    );
    expect(screen.getByText('one')).toBeInTheDocument();
    expect(screen.getByText('two')).toBeInTheDocument();
    const notice = getThemeNotice();
    expect(notice).toEqual({
      kind: 'error',
      message: 'The theme "ocean" has a component that failed - the default is shown.',
    });
    // closing the note does not bring it back for the same theme
    setThemeNotice(undefined);
    render(
      <SlotBoundary fallback={<span>three</span>} themeId="ocean" where="the slot Panel">
        <Broken />
      </SlotBoundary>,
    );
    expect(screen.getByText('three')).toBeInTheDocument();
    expect(getThemeNotice()).toBeUndefined();
  });

  it('tries the theme component again when the theme changes', () => {
    const { rerender } = render(
      <SlotBoundary fallback={<span>built-in</span>} themeId="ocean" where="the slot Button">
        <Broken />
      </SlotBoundary>,
    );
    expect(screen.getByText('built-in')).toBeInTheDocument();
    rerender(
      <SlotBoundary fallback={<span>built-in</span>} themeId="forest" where="the slot Button">
        <span>themed</span>
      </SlotBoundary>,
    );
    expect(screen.getByText('themed')).toBeInTheDocument();
  });

  it('keeps the fallback while the same theme rerenders', () => {
    const { rerender } = render(
      <SlotBoundary fallback={<span>built-in</span>} themeId="ocean" where="the slot Button">
        <Broken />
      </SlotBoundary>,
    );
    rerender(
      <SlotBoundary fallback={<span>built-in</span>} themeId="ocean" where="the slot Button">
        <span>themed</span>
      </SlotBoundary>,
    );
    expect(screen.getByText('built-in')).toBeInTheDocument();
  });
});
