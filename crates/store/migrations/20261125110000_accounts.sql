-- Claude logins that runs are spread over; the default row is the server's own login
CREATE TABLE claude_accounts (
  id                  BLOB PRIMARY KEY CHECK (length(id) = 16),
  slug                TEXT NOT NULL UNIQUE
                      CHECK (length(slug) BETWEEN 1 AND 40 AND slug GLOB '[a-z0-9]*' AND slug NOT GLOB '*[^a-z0-9-]*'),
  name                TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 60),
  config_dir          TEXT CHECK (config_dir IS NULL OR length(config_dir) BETWEEN 1 AND 1000),
  enabled             INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  is_fallback         INTEGER NOT NULL DEFAULT 0 CHECK (is_fallback IN (0, 1)),
  position            INTEGER NOT NULL CHECK (position >= 0),
  limit_five_hour_pct INTEGER CHECK (limit_five_hour_pct BETWEEN 1 AND 100),
  limit_week_pct      INTEGER CHECK (limit_week_pct BETWEEN 1 AND 100),
  token_secret_ref    TEXT CHECK (token_secret_ref IS NULL OR length(token_secret_ref) BETWEEN 1 AND 200),
  created_at          INTEGER NOT NULL,
  updated_at          INTEGER NOT NULL,
  version             INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  CHECK (slug <> 'default' OR config_dir IS NULL)
) STRICT;

INSERT INTO claude_accounts (id, slug, name, position, created_at, updated_at)
VALUES (x'019a0000000070008000000000000001', 'default', 'Default', 0,
        CAST(unixepoch('subsec') * 1000 AS INTEGER), CAST(unixepoch('subsec') * 1000 AS INTEGER));
