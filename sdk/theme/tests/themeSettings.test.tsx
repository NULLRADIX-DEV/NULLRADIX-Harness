import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ThemeHostProvider, useThemeSettings } from '../src/index.js';
import type { ThemeDefinition, ThemeHost, ThemeSettingsSchema } from '../src/index.js';

afterEach(cleanup);

const schema = {
  sounds: { type: 'boolean', label: 'Sounds', default: true },
  style: {
    type: 'select',
    label: 'Style',
    options: [{ value: 'pixel', label: 'Pixel' }],
    default: 'pixel',
  },
} satisfies ThemeSettingsSchema;

// Theme component with imports from the SDK and react only
function Sounds() {
  const { values, set } = useThemeSettings<typeof schema>();
  const on: boolean = values.sounds;
  const style: string = values.style;
  return (
    <button onClick={() => void set('sounds', false)}>{`${on ? 'on' : 'off'}|${style}`}</button>
  );
}

describe('useThemeSettings', () => {
  it('reads the values and saves a change through the host', () => {
    const set = vi.fn(() => Promise.resolve());
    const host = {
      themeSettings: { values: { sounds: true, style: 'pixel' }, loading: false, set },
    } as unknown as ThemeHost;
    render(
      <ThemeHostProvider value={host}>
        <Sounds />
      </ThemeHostProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'on|pixel' }));
    expect(set).toHaveBeenCalledWith('sounds', false);
  });

  it('throws without a provider and names the hook', () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    expect(() => render(<Sounds />)).toThrow(/useThemeSettings needs the ThemeHostProvider/);
  });

  it('ThemeDefinition takes a settingsSchema', () => {
    const theme = {
      id: 'with-schema',
      slots: {},
      settingsSchema: schema,
    } satisfies ThemeDefinition;
    expect(Object.keys(theme.settingsSchema)).toEqual(['sounds', 'style']);
  });
});
