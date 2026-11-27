import { act, render, screen } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { getThemeNotice, setThemeNotice } from '../lib/themeNotice';
import { ThemeNotice } from './ThemeNotice';

afterEach(() => {
  setThemeNotice(undefined);
  window.history.replaceState(null, '', '/');
});

describe('ThemeNotice', () => {
  it('shows nothing until there is a note and can be closed', async () => {
    render(<ThemeNotice />);
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    act(() => {
      setThemeNotice('Theme broken');
    });
    expect(screen.getByRole('status')).toHaveTextContent('Theme broken');
    await userEvent.click(screen.getByRole('button', { name: 'Close the note' }));
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(getThemeNotice()).toBeUndefined();
  });

  it('offers to reload in safe mode after an error', async () => {
    window.history.replaceState(null, '', '/board?a=2#x');
    const assign = vi.fn();
    render(<ThemeNotice assign={assign} />);
    act(() => {
      setThemeNotice('Theme broken');
    });
    await userEvent.click(screen.getByRole('button', { name: 'Reload in safe mode' }));
    expect(assign).toHaveBeenCalledWith(`${window.location.origin}/board?a=2&safe-theme#x`);
    expect(screen.queryByRole('button', { name: 'Start normally' })).not.toBeInTheDocument();
  });

  it('offers the way back in safe mode and no second safe mode', async () => {
    window.history.replaceState(null, '', '/board?a=2&safe-theme#x');
    const assign = vi.fn();
    render(<ThemeNotice assign={assign} />);
    act(() => {
      setThemeNotice({ kind: 'error', message: 'Theme broken' });
    });
    expect(screen.queryByRole('button', { name: 'Reload in safe mode' })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Start normally' }));
    expect(assign).toHaveBeenCalledWith(`${window.location.origin}/board?a=2#x`);
  });

  it('does not offer safe mode for information', () => {
    render(<ThemeNotice />);
    act(() => {
      setThemeNotice({ kind: 'info', message: 'Safe mode: code is off.' });
    });
    expect(screen.queryByRole('button', { name: 'Reload in safe mode' })).not.toBeInTheDocument();
  });
});
