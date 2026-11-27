import type { ThemeSlots } from '@nullradix/theme-sdk';
import { act, screen } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { SettingsPage } from '../../features/settings/SettingsPage';
import { ThemesSection } from '../../features/themes/ThemesSection';
import { renderThemed } from '../../test/renderThemed';
import { setThemeNotice } from '../lib/themeNotice';
import { ThemeNotice } from './ThemeNotice';

// A theme that renders nothing useful: buttons ignore clicks, panels drop their content.
const HOSTILE: ThemeSlots = {
  Button: () => <span>dead button</span>,
  Panel: () => <span>empty panel</span>,
  Card: () => null,
  Dialog: () => null,
  Input: () => null,
};

beforeEach(() => {
  vi.spyOn(console, 'error').mockImplementation(() => undefined);
});

describe('the way out of theme code never runs through slots', () => {
  it('keeps the buttons of the notice working under a theme Button that ignores clicks', async () => {
    const assign = vi.fn();
    renderThemed(<ThemeNotice assign={assign} />, { id: 'ocean', slots: HOSTILE });
    act(() => {
      setThemeNotice('Theme broken');
    });
    await userEvent.click(screen.getByRole('button', { name: 'Reload in safe mode' }));
    expect(assign).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole('button', { name: 'Close the note' }));
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('keeps the way back from safe mode working too', async () => {
    window.history.replaceState(null, '', '/?safe-theme');
    const assign = vi.fn();
    renderThemed(<ThemeNotice assign={assign} />, { id: 'ocean', slots: HOSTILE });
    act(() => {
      setThemeNotice({ kind: 'info', message: 'Safe mode' });
    });
    await userEvent.click(screen.getByRole('button', { name: 'Start normally' }));
    expect(assign).toHaveBeenCalledTimes(1);
    window.history.replaceState(null, '', '/');
  });

  it('keeps the code switch of a theme under a theme Panel that renders nothing', () => {
    renderThemed(<ThemesSection />, { id: 'ocean', slots: HOSTILE });
    expect(screen.getByRole('switch', { name: 'Run the code of ocean' })).toBeInTheDocument();
    expect(screen.queryByText('empty panel')).not.toBeInTheDocument();
  });

  it('keeps the theme choice under a theme Panel that renders nothing', () => {
    renderThemed(<SettingsPage />, { id: 'ocean', slots: HOSTILE });
    expect(screen.getByRole('combobox', { name: 'Theme' })).toBeInTheDocument();
  });
});
