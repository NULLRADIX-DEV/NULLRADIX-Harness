-- The branch commit at the start of a merge session; none for jobs without one
ALTER TABLE merge_jobs ADD COLUMN session_branch_head TEXT;
