import type { ReactNode } from 'react';

function LineIcon({ children }: { children: ReactNode }) {
  return (
    <svg
      className="nav-icon"
      width="18"
      height="18"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  );
}

export function BoardIcon() {
  return (
    <LineIcon>
      <path d="M4 4h6v16H4zM14 4h6v10h-6z" />
    </LineIcon>
  );
}

export function InboxIcon() {
  return (
    <LineIcon>
      <path d="M4 13l2.5-8h11L20 13v6H4zM4 13h5l1 2h4l1-2h5" />
    </LineIcon>
  );
}

export function DashboardIcon() {
  return (
    <LineIcon>
      <path d="M4 20V10M10 20V4M16 20v-7M21 20H3" />
    </LineIcon>
  );
}

export function QuestionsIcon() {
  return (
    <LineIcon>
      <path d="M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .9-1 1.6v.6M12 17h.01" />
    </LineIcon>
  );
}

export function SessionsIcon() {
  return (
    <LineIcon>
      <path d="M4 5h16v14H4zM8 10l3 2-3 2M13 14h3" />
    </LineIcon>
  );
}

export function SettingsIcon() {
  return (
    <LineIcon>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3h.1a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8v.1a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" />
    </LineIcon>
  );
}
