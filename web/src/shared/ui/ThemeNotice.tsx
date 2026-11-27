import { isSafeTheme, normalThemeUrl, safeThemeUrl } from '../lib/safeTheme';
import { setThemeNotice, useThemeNotice } from '../lib/themeNotice';
import './themeNotice.css';
import { Button } from './Button';

// Closable bar about the theme; a theme error offers safe mode, and safe mode offers the way back.
export function ThemeNotice({
  assign = (url) => {
    window.location.assign(url);
  },
}: {
  assign?: (url: string) => void;
}) {
  const notice = useThemeNotice();
  if (!notice) return null;
  const safe = isSafeTheme();
  return (
    <div className="theme-notice" role="status">
      <span>{notice.message}</span>
      {notice.kind === 'error' && !safe && (
        <Button
          type="button"
          onClick={() => {
            assign(safeThemeUrl(window.location.href));
          }}
        >
          Reload in safe mode
        </Button>
      )}
      {safe && (
        <Button
          type="button"
          onClick={() => {
            assign(normalThemeUrl(window.location.href));
          }}
        >
          Start normally
        </Button>
      )}
      <Button
        type="button"
        aria-label="Close the note"
        onClick={() => {
          setThemeNotice(undefined);
        }}
      >
        x
      </Button>
    </div>
  );
}
