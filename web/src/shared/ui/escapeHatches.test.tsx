import type { ThemeSlots } from '@nullradix/theme-sdk';
import { act, screen, waitFor, within } from '@testing-library/react';
import { userEvent } from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { SettingsPage } from '../../features/settings/SettingsPage';
import { CatalogSection } from '../../features/themes/CatalogSection';
import { ClaudeThemeSection } from '../../features/themes/ClaudeThemeSection';
import { ThemeConfirmDialog } from '../../features/themes/ThemeConfirmDialog';
import { ThemesSection } from '../../features/themes/ThemesSection';
import type { Confirmation } from '../api/errors';
import { json } from '../../test/fakeApi';
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

const HOSTILE_THEME = { id: 'ocean', slots: HOSTILE };

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

  it('keeps the confirmation of a risky install out of the slot Dialog', async () => {
    const facts: Confirmation = {
      hasCode: true,
      domains: ['cdn.example.com'],
      claudeTheme: false,
      update: false,
      id: 'night-blue',
      name: 'Night Blue',
      version: '1.0.0',
      sha256: null,
      added: { code: false, claudeTheme: false, domains: [] },
    };
    const onCancel = vi.fn();
    const onConfirm = vi.fn();
    renderThemed(
      <ThemeConfirmDialog facts={facts} busy={false} onConfirm={onConfirm} onCancel={onCancel} />,
      { id: 'ocean', slots: { Dialog: () => <p>nothing to see here</p> } },
    );
    expect(screen.getByText(/This package contains code/)).toBeInTheDocument();
    expect(screen.getByRole('list', { name: 'Network domains' })).toHaveTextContent(
      'cdn.example.com',
    );
    expect(screen.queryByText('nothing to see here')).not.toBeInTheDocument();
    expect(screen.getByRole('alertdialog')).not.toHaveAttribute('data-slot');
    expect(screen.getByRole('button', { name: 'Install' })).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it('keeps the Claude part switch under a hostile theme', async () => {
    const put = vi.fn();
    const status = {
      enabled: false,
      state: 'none',
      themeId: 'ocean',
      themeName: 'Ocean',
      targets: [],
    };
    renderThemed(<ClaudeThemeSection />, HOSTILE_THEME, {
      routes: {
        'GET /api/v1/settings/claude-theme': () => json(200, status),
        'PUT /api/v1/settings/claude-theme': () => {
          put();
          return json(200, { ...status, enabled: true });
        },
      },
    });
    const toggle = await screen.findByRole('switch', { name: 'Let Claude use the chosen theme' });
    await waitFor(() => {
      expect(toggle).toBeEnabled();
    });
    await userEvent.click(toggle);
    await waitFor(() => {
      expect(put).toHaveBeenCalledTimes(1);
    });
    expect(screen.queryByText('empty panel')).not.toBeInTheDocument();
  });

  it('keeps install, Replace and Cancel of the catalog under a hostile theme', async () => {
    const offered = (id: string, name: string, state: string) => ({
      sourceId: 'aaaaaaaaaaaa',
      id,
      name,
      version: '1.0.0',
      sdkVersion: '^2.0',
      compatible: true,
      description: null,
      state,
      installedVersion: state === 'other-source' ? '0.9.0' : null,
    });
    renderThemed(<CatalogSection />, HOSTILE_THEME, {
      routes: {
        'GET /api/v1/theme-catalog/sources': () =>
          json(200, {
            items: [
              { id: 'aaaaaaaaaaaa', url: 'https://themes.example.com/index.json', addedAt: 1 },
            ],
            version: 1,
          }),
        'GET /api/v1/theme-catalog/themes': () =>
          json(200, {
            items: [
              offered('night-blue', 'Night Blue', 'available'),
              offered('forest', 'Forest', 'other-source'),
            ],
            sources: [],
          }),
      },
    });
    expect(await screen.findByRole('button', { name: 'Install Night Blue' })).toBeInTheDocument();
    expect(screen.getByRole('textbox', { name: 'Add a source' })).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Replace Forest' }));
    const group = within(screen.getByRole('group', { name: 'Replace Forest' }));
    expect(group.getByRole('button', { name: 'Replace' })).toBeInTheDocument();
    await userEvent.click(group.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('group', { name: 'Replace Forest' })).not.toBeInTheDocument();
    expect(screen.queryByText('dead button')).not.toBeInTheDocument();
  });
});
