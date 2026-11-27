-- small stylesheets a person adds on top of the active theme
CREATE TABLE css_snippets (
  snippet_id TEXT PRIMARY KEY CHECK (length(snippet_id) BETWEEN 2 AND 40),
  -- entity id in the change feed, which only takes UUIDs
  feed_id    BLOB NOT NULL UNIQUE CHECK (length(feed_id) = 16),
  name       TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 1000),
  css        TEXT NOT NULL CHECK (length(CAST(css AS BLOB)) <= 65536),
  enabled    INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  -- position in the delivered stylesheet
  sort_order INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  version    INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1)
) STRICT;
