import { useQuery } from '@tanstack/react-query';
import { useEffect } from 'react';
import { applySnippetsStylesheet, snippetsVersion } from '../../shared/lib/snippets';
import { snippetsQuery } from './queries';

// Keeps the link to the active snippets in step with the list; shows nothing itself.
export function SnippetsStylesheet() {
  const snippets = useQuery(snippetsQuery());
  const version = snippets.data ? snippetsVersion(snippets.data) : undefined;

  useEffect(() => {
    // while the list loads or fails, the stylesheet stays as it is
    if (version === undefined) return;
    applySnippetsStylesheet(document, version);
  }, [version]);

  return null;
}
