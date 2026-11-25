-- the package files live in the data folder; this table tracks which themes are installed
CREATE TABLE installed_themes (
  theme_id         TEXT PRIMARY KEY CHECK (length(theme_id) BETWEEN 2 AND 40),
  -- entity id in the change feed, which only takes UUIDs
  feed_id          BLOB NOT NULL UNIQUE CHECK (length(feed_id) = 16),
  name             TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 1000),
  theme_version    TEXT NOT NULL CHECK (length(theme_version) BETWEEN 1 AND 100),
  author           TEXT NOT NULL CHECK (length(author) BETWEEN 1 AND 1000),
  sdk_version      TEXT NOT NULL CHECK (length(sdk_version) BETWEEN 1 AND 100),
  color_scheme     TEXT NOT NULL CHECK (color_scheme IN ('light', 'dark')),
  has_claude_theme INTEGER NOT NULL CHECK (has_claude_theme IN (0, 1)),
  manifest         TEXT NOT NULL CHECK (json_valid(manifest)),
  installed_at     INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL,
  version          INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1)
) STRICT;
