import type { ReactNode } from 'react';

export function ComingSoon({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="page">
      <h1 className="page-title">{title}</h1>
      <div className="state empty">
        <h2>Coming soon</h2>
        <p>This page is being rebuilt.</p>
      </div>
      {children}
    </div>
  );
}
