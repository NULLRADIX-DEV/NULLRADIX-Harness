-- the saved order of the inbox; ids point at tickets, questions or notes, stale ones are skipped
CREATE TABLE inbox_order (
  position  INTEGER PRIMARY KEY CHECK (position BETWEEN 0 AND 999),
  item_type TEXT NOT NULL CHECK (item_type IN ('ticket', 'question', 'note')),
  item_id   BLOB NOT NULL CHECK (length(item_id) = 16),
  UNIQUE (item_type, item_id)
) STRICT;
