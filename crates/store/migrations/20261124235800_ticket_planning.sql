ALTER TABLE tickets ADD COLUMN planned_by TEXT
  CHECK (planned_by IS NULL OR length(planned_by) BETWEEN 1 AND 100);
ALTER TABLE tickets ADD COLUMN planned_at INTEGER
  CHECK ((planned_at IS NULL) = (planned_by IS NULL));
ALTER TABLE tickets ADD COLUMN planning_claim_by TEXT
  CHECK (planning_claim_by IS NULL OR length(planning_claim_by) BETWEEN 1 AND 100);
ALTER TABLE tickets ADD COLUMN planning_claim_at INTEGER
  CHECK ((planning_claim_at IS NULL) = (planning_claim_by IS NULL));
