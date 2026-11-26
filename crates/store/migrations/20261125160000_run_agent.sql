-- The agent a dispatch run worked under; old rows have none
ALTER TABLE runs ADD COLUMN agent TEXT;
