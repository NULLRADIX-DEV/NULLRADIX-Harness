import type { SessionClient } from '../../api/sessionDetailTypes.js';

export const CLIENT_LABEL: Record<SessionClient, string> = {
  nullradix: 'NULLRADIX',
  'claude-code': 'Claude Code',
  'claude-desktop': 'Desktop',
  unknown: 'Unbekannt',
};

const CLIENT_ICON: Record<SessionClient, string> = {
  nullradix: 'hub',
  'claude-code': 'terminal',
  'claude-desktop': 'desktop_windows',
  unknown: 'help_outline',
};

// Quelle einer Session als Marke mit Symbol und Text (NULLRADIX, Claude Code, Desktop)
export function SessionClientBadge({ client }: { client: SessionClient }) {
  return (
    <span className={`badge client-badge client-${client}`} title={`Quelle: ${CLIENT_LABEL[client]}`}>
      <span className="material-icons" aria-hidden="true">
        {CLIENT_ICON[client]}
      </span>
      {CLIENT_LABEL[client]}
    </span>
  );
}
