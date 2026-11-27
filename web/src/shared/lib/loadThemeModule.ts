import { SDK_VERSION, type ThemeDefinition } from '@nullradix/theme-sdk';
import type { ThemeList } from '../query/themes';
import { satisfiesSdkRange } from './sdkRange';
import { isSafeTheme } from './safeTheme';
import { isThemeMode, readThemeChoice } from './theme';
import { setThemeNotice } from './themeNotice';
import { registerTheme } from './themeRegistry';

// What loading the code of the chosen theme came to.
export interface ThemeLoadResult {
  status: 'loaded' | 'skipped' | 'incompatible' | 'timeout' | 'failed' | 'invalid';
  themeId?: string;
}

// Replaceable parts, so tests bring their own.
export interface ThemeModuleDeps {
  fetchThemes: () => Promise<ThemeList>;
  importModule?: (url: string) => Promise<unknown>;
  timeoutMs?: number;
  sdkVersion?: string;
  search?: string;
  storage?: Pick<Storage, 'getItem'>;
}

// Time for the theme list and the module together.
export const LOAD_TIMEOUT_MS = 3000;

class TimeoutError extends Error {}

function fail(status: ThemeLoadResult['status'], themeId: string, message: string) {
  console.warn(message);
  setThemeNotice(message);
  return { status, themeId } satisfies ThemeLoadResult;
}

function isDefinition(value: unknown, id: string): value is ThemeDefinition {
  if (typeof value !== 'object' || value === null) return false;
  const { id: found, slots } = value as { id?: unknown; slots?: unknown };
  return found === id && typeof slots === 'object' && slots !== null;
}

// Before the first render: runs main.js of the chosen theme if it has code and its switch is on.
export async function loadThemeModule(deps: ThemeModuleDeps): Promise<ThemeLoadResult> {
  const sdkVersion = deps.sdkVersion ?? SDK_VERSION;
  const importModule =
    deps.importModule ?? ((url: string) => import(/* @vite-ignore */ url) as Promise<unknown>);
  const chosen = readThemeChoice(deps.storage ?? window.localStorage);
  const installedChoice = isThemeMode(chosen) ? undefined : chosen;

  if (isSafeTheme(deps.search)) {
    if (installedChoice === undefined) return { status: 'skipped' };
    setThemeNotice({
      kind: 'info',
      message: `Safe mode: code and CSS of the theme "${installedChoice}" are off.`,
    });
    return { status: 'skipped', themeId: installedChoice };
  }

  // after the limit nothing arrives any more; a late result is dropped
  const limit = { passed: false };
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      limit.passed = true;
      reject(new TimeoutError());
    }, deps.timeoutMs ?? LOAD_TIMEOUT_MS);
  });

  let themeId = installedChoice;
  try {
    let list: ThemeList;
    try {
      list = await Promise.race([deps.fetchThemes(), timeout]);
    } catch (error) {
      if (error instanceof TimeoutError || installedChoice === undefined) throw error;
      const reason = error instanceof Error ? error.message : String(error);
      return fail(
        'failed',
        installedChoice,
        `The theme "${installedChoice}" could not be loaded (the theme list is unreadable: ${reason}) - the default is shown.`,
      );
    }
    const theme = list.items.find((item) => item.id === list.active);
    const codeUrl = theme?.codeUrl;
    if (!theme?.hasCode || !theme.codeEnabled || !codeUrl) {
      return { status: 'skipped', ...(theme && { themeId: theme.id }) };
    }
    themeId = theme.id;
    if (!satisfiesSdkRange(sdkVersion, theme.sdkVersion)) {
      return fail(
        'incompatible',
        theme.id,
        `The theme "${theme.id}" does not fit this NULLRADIX version (SDK ${sdkVersion}, needs ${theme.sdkVersion}) - the default is shown.`,
      );
    }
    let module: unknown;
    // a module past the limit is dropped, but its top-level code may still run
    try {
      module = await Promise.race([importModule(codeUrl), timeout]);
    } catch (error) {
      if (error instanceof TimeoutError) throw error;
      return fail(
        'failed',
        theme.id,
        `The theme "${theme.id}" could not be loaded (${String(error)}) - the default is shown.`,
      );
    }
    if (limit.passed) throw new TimeoutError();
    const definition = (module as { default?: unknown } | null)?.default;
    if (!isDefinition(definition, theme.id)) {
      return fail(
        'invalid',
        theme.id,
        `The theme "${theme.id}" delivers no valid theme definition - the default is shown.`,
      );
    }
    registerTheme(definition);
    return { status: 'loaded', themeId: theme.id };
  } catch (error) {
    // no theme was asked for and the list failed: the default needs no word
    if (themeId === undefined) return { status: 'skipped' };
    if (error instanceof TimeoutError) {
      return fail(
        'timeout',
        themeId,
        `The theme "${themeId}" was not loaded in time - the default is shown.`,
      );
    }
    return fail(
      'failed',
      themeId,
      `The theme "${themeId}" could not be loaded (${String(error)}) - the default is shown.`,
    );
  } finally {
    clearTimeout(timer);
  }
}
