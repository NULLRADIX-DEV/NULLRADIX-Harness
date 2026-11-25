-- Dispatcher configuration: one settings row, profiles, rules, ladder, setup steps and templates
CREATE TABLE dispatch_profiles (
  id              BLOB PRIMARY KEY CHECK (length(id) = 16),
  name            TEXT NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 60),
  model           TEXT,
  effort          TEXT,
  max_turns       INTEGER NOT NULL CHECK (max_turns > 0),
  timeout_minutes INTEGER NOT NULL CHECK (timeout_minutes > 0),
  position        INTEGER NOT NULL CHECK (position >= 0),
  retired         INTEGER NOT NULL DEFAULT 0 CHECK (retired IN (0, 1)),
  version         INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1)
) STRICT;

CREATE TABLE dispatcher_settings (
  id                           INTEGER PRIMARY KEY CHECK (id = 1),
  enabled                      INTEGER NOT NULL CHECK (enabled IN (0, 1)),
  wip                          INTEGER NOT NULL CHECK (wip >= 0),
  window_from                  TEXT CHECK (window_from IS NULL OR window_from GLOB '[0-2][0-9]:[0-5][0-9]'),
  window_to                    TEXT CHECK (window_to IS NULL OR window_to GLOB '[0-2][0-9]:[0-5][0-9]'),
  opt_in_label                 TEXT,
  opt_out_label                TEXT,
  agent_prefix                 TEXT NOT NULL CHECK (length(agent_prefix) > 0),
  dispatcher_actor_id          BLOB NOT NULL REFERENCES actors (id),
  stale_minutes                INTEGER NOT NULL CHECK (stale_minutes > 0),
  max_attempts                 INTEGER NOT NULL CHECK (max_attempts >= 1),
  pause_minutes_on_limit       INTEGER NOT NULL CHECK (pause_minutes_on_limit >= 1),
  prompt_extra                 TEXT NOT NULL DEFAULT '',
  merge_strategy               TEXT NOT NULL CHECK (merge_strategy IN ('merge', 'squash')),
  review_before_merge          INTEGER NOT NULL CHECK (review_before_merge IN (0, 1)),
  join_worktrees               TEXT NOT NULL CHECK (join_worktrees IN ('none', 'same-spec')),
  worktree_setup_timeout_min   INTEGER NOT NULL CHECK (worktree_setup_timeout_min > 0),
  claude_command               TEXT NOT NULL CHECK (length(claude_command) > 0),
  claude_model                 TEXT,
  claude_effort                TEXT,
  claude_max_turns             INTEGER NOT NULL CHECK (claude_max_turns > 0),
  claude_timeout_minutes       INTEGER NOT NULL CHECK (claude_timeout_minutes > 0),
  claude_permission_mode       TEXT,
  claude_allowed_tools         TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(claude_allowed_tools)),
  claude_extra_args            TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(claude_extra_args)),
  claude_use_subscription      INTEGER NOT NULL CHECK (claude_use_subscription IN (0, 1)),
  git_bash_path                TEXT,
  default_profile_id           BLOB NOT NULL REFERENCES dispatch_profiles (id),
  escalate                     INTEGER NOT NULL CHECK (escalate IN (0, 1)),
  allow_fable                  INTEGER NOT NULL CHECK (allow_fable IN (0, 1)),
  research_auto                INTEGER NOT NULL CHECK (research_auto IN (0, 1)),
  research_wip                 INTEGER NOT NULL CHECK (research_wip BETWEEN 1 AND 5),
  research_max_per_day         INTEGER NOT NULL CHECK (research_max_per_day >= 1),
  research_profile_id          BLOB REFERENCES dispatch_profiles (id),
  planning_auto                INTEGER NOT NULL CHECK (planning_auto IN (0, 1)),
  planning_wip                 INTEGER NOT NULL CHECK (planning_wip BETWEEN 1 AND 5),
  planning_acceptance          INTEGER NOT NULL CHECK (planning_acceptance IN (0, 1)),
  planning_profile_id          BLOB REFERENCES dispatch_profiles (id),
  autopilot_enabled            INTEGER NOT NULL CHECK (autopilot_enabled IN (0, 1)),
  autopilot_except_label       TEXT,
  autopilot_max_merge_sessions INTEGER NOT NULL CHECK (autopilot_max_merge_sessions BETWEEN 1 AND 5),
  account_strategy             TEXT NOT NULL CHECK (account_strategy IN ('most-free', 'order')),
  account_limit_pct            INTEGER NOT NULL CHECK (account_limit_pct BETWEEN 50 AND 100),
  account_reserve_five_hour    INTEGER NOT NULL CHECK (account_reserve_five_hour BETWEEN 0 AND 50),
  account_reserve_week         INTEGER NOT NULL CHECK (account_reserve_week BETWEEN 0 AND 50),
  account_reset_grace_min      INTEGER NOT NULL CHECK (account_reset_grace_min BETWEEN 0 AND 300),
  config_hash                  TEXT NOT NULL,
  updated_at                   INTEGER NOT NULL,
  version                      INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  CHECK ((window_from IS NULL) = (window_to IS NULL))
) STRICT;

CREATE TABLE dispatch_profile_accounts (
  profile_id BLOB NOT NULL REFERENCES dispatch_profiles (id) ON DELETE CASCADE,
  account_id BLOB NOT NULL REFERENCES claude_accounts (id) ON DELETE CASCADE,
  position   INTEGER NOT NULL CHECK (position >= 0),
  PRIMARY KEY (profile_id, account_id)
) STRICT;

CREATE TABLE dispatch_rules (
  id                  BLOB PRIMARY KEY CHECK (length(id) = 16),
  position            INTEGER NOT NULL UNIQUE CHECK (position >= 1),
  profile_id          BLOB NOT NULL REFERENCES dispatch_profiles (id),
  when_labels         TEXT CHECK (when_labels IS NULL OR json_valid(when_labels)),
  when_priorities     TEXT CHECK (when_priorities IS NULL OR json_valid(when_priorities)),
  when_files_min      INTEGER CHECK (when_files_min >= 1),
  when_plan_min_chars INTEGER CHECK (when_plan_min_chars >= 1),
  version             INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  CHECK (when_labels IS NOT NULL OR when_priorities IS NOT NULL
         OR when_files_min IS NOT NULL OR when_plan_min_chars IS NOT NULL)
) STRICT;

CREATE TABLE dispatch_rule_accounts (
  rule_id    BLOB NOT NULL REFERENCES dispatch_rules (id) ON DELETE CASCADE,
  account_id BLOB NOT NULL REFERENCES claude_accounts (id) ON DELETE CASCADE,
  position   INTEGER NOT NULL CHECK (position >= 0),
  PRIMARY KEY (rule_id, account_id)
) STRICT;

CREATE TABLE dispatch_escalation_steps (
  position   INTEGER PRIMARY KEY CHECK (position >= 0),
  profile_id BLOB NOT NULL UNIQUE REFERENCES dispatch_profiles (id) ON DELETE CASCADE
) STRICT;

CREATE TABLE worktree_setup_steps (
  position INTEGER PRIMARY KEY CHECK (position >= 0),
  cwd      TEXT NOT NULL CHECK (length(cwd) > 0),
  command  TEXT NOT NULL CHECK (length(command) > 0)
) STRICT;

CREATE TABLE prompt_templates (
  kind       TEXT PRIMARY KEY CHECK (kind IN ('work', 'plan', 'research', 'merge')),
  template   TEXT NOT NULL CHECK (length(trim(template)) > 0),
  updated_at INTEGER NOT NULL,
  version    INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1)
) STRICT;

CREATE TABLE dispatch_config_versions (
  hash          TEXT PRIMARY KEY CHECK (length(hash) = 16),
  content       TEXT NOT NULL CHECK (json_valid(content)),
  first_seen_at INTEGER NOT NULL
) STRICT;

CREATE INDEX ix_config_versions_seen ON dispatch_config_versions (first_seen_at);

INSERT OR IGNORE INTO actors (id, kind, name, created_at)
VALUES (x'019a0000000070008000000000000003', 'dispatcher', 'dispatcher',
        CAST(unixepoch('subsec') * 1000 AS INTEGER));

INSERT INTO dispatch_profiles (id, name, model, effort, max_turns, timeout_minutes, position) VALUES
  (x'019a0000000070008000000000000010', 'small', 'sonnet', 'high', 40, 45, 0),
  (x'019a0000000070008000000000000011', 'medium', 'opus', 'high', 80, 90, 1),
  (x'019a0000000070008000000000000012', 'large', 'opus', 'max', 150, 120, 2);

INSERT INTO dispatch_escalation_steps (position, profile_id) VALUES
  (0, x'019a0000000070008000000000000010'),
  (1, x'019a0000000070008000000000000011'),
  (2, x'019a0000000070008000000000000012');

INSERT INTO dispatch_rules (id, position, profile_id, when_labels, when_priorities, when_files_min) VALUES
  (x'019a0000000070008000000000000020', 1, x'019a0000000070008000000000000010', '["docs","typo","small"]', NULL, NULL),
  (x'019a0000000070008000000000000021', 2, x'019a0000000070008000000000000012', '["architecture","refactoring"]', NULL, NULL),
  (x'019a0000000070008000000000000022', 3, x'019a0000000070008000000000000012', NULL, '["urgent"]', NULL),
  (x'019a0000000070008000000000000023', 4, x'019a0000000070008000000000000012', NULL, NULL, 6);

INSERT INTO dispatcher_settings (
  id, enabled, wip, window_from, window_to, opt_in_label, opt_out_label, agent_prefix,
  dispatcher_actor_id, stale_minutes, max_attempts, pause_minutes_on_limit, prompt_extra,
  merge_strategy, review_before_merge, join_worktrees, worktree_setup_timeout_min,
  claude_command, claude_model, claude_effort, claude_max_turns, claude_timeout_minutes,
  claude_permission_mode, claude_allowed_tools, claude_extra_args, claude_use_subscription,
  git_bash_path, default_profile_id, escalate, allow_fable,
  research_auto, research_wip, research_max_per_day, research_profile_id,
  planning_auto, planning_wip, planning_acceptance, planning_profile_id,
  autopilot_enabled, autopilot_except_label, autopilot_max_merge_sessions,
  account_strategy, account_limit_pct, account_reserve_five_hour, account_reserve_week,
  account_reset_grace_min, config_hash, updated_at
) VALUES (
  1, 0, 1, NULL, NULL, 'auto', 'manual', 'worker-',
  (SELECT id FROM actors WHERE kind = 'dispatcher' AND name = 'dispatcher'), 150, 2, 60, '',
  'merge', 1, 'none', 15,
  'claude', 'sonnet', NULL, 80, 60,
  'acceptEdits',
  '["Read","Edit","Write","Glob","Grep","Bash(harness:*)","Bash(nullradix:*)","Bash(node:*)","Bash(npm test:*)","Bash(npm run:*)","Bash(git status:*)","Bash(git diff:*)","Bash(git log:*)","Bash(git branch:*)","Bash(git add:*)","Bash(git commit:*)"]',
  '[]', 1,
  NULL, x'019a0000000070008000000000000011', 1, 0,
  0, 1, 10, NULL,
  0, 1, 1, NULL,
  0, 'interactive', 3,
  'most-free', 90, 10, 2,
  30, '', CAST(unixepoch('subsec') * 1000 AS INTEGER)
);
