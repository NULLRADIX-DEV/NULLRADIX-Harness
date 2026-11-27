import { isSafeTheme, normalThemeUrl, safeThemeUrl } from '../lib/safeTheme';
import { setThemeNotice, useThemeNotice } from '../lib/themeNotice';
import './themeNotice.css';

// Closable bar offering safe mode and the way back; plain elements, never slots.
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
        <button
          type="button"
          className="btn"
          onClick={() => {
            assign(safeThemeUrl(window.location.href));
          }}
        >
          Reload in safe mode
        </button>
      )}
      {safe && (
        <button
          type="button"
          className="btn"
          onClick={() => {
            assign(normalThemeUrl(window.location.href));
          }}
        >
          Start normally
        </button>
      )}
      <button
        type="button"
        className="btn"
        aria-label="Close the note"
        onClick={() => {
          setThemeNotice(undefined);
        }}
      >
        x
      </button>
    </div>
  );
}
