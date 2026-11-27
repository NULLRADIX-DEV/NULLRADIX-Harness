import { setThemeNotice } from './themeNotice';

const reported = new Set<string>();

// A theme component failed: log it every time, tell the person once per theme.
export function reportThemeFailure(
  themeId: string,
  where: string,
  error: unknown,
  componentStack?: string | null,
): void {
  console.error(
    `Theme "${themeId}": ${where} failed, the default is shown.`,
    error,
    componentStack,
  );
  if (reported.has(themeId)) return;
  reported.add(themeId);
  setThemeNotice(`The theme "${themeId}" has a component that failed - the default is shown.`);
}

export function forgetThemeFailures(): void {
  reported.clear();
}
