-- Readings of an account's limits; the start and end rows of a run feed the reserve estimate
CREATE TABLE account_usage_samples (
  id                  BLOB PRIMARY KEY CHECK (length(id) = 16),
  account_id          BLOB NOT NULL REFERENCES claude_accounts (id) ON DELETE CASCADE,
  run_id              BLOB REFERENCES runs (id) ON DELETE SET NULL,
  phase               TEXT NOT NULL CHECK (phase IN ('start', 'end', 'poll')),
  five_hour_pct       INTEGER CHECK (five_hour_pct IS NULL OR five_hour_pct >= 0),
  seven_day_pct       INTEGER CHECK (seven_day_pct IS NULL OR seven_day_pct >= 0),
  five_hour_resets_at INTEGER,
  seven_day_resets_at INTEGER,
  at                  INTEGER NOT NULL
) STRICT;

CREATE INDEX ix_usage_samples_account_at ON account_usage_samples (account_id, at);
CREATE UNIQUE INDEX ux_usage_samples_run_phase ON account_usage_samples (run_id, phase)
  WHERE run_id IS NOT NULL AND phase <> 'poll';
