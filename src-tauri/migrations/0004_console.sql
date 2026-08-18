-- Per-instance command history for the console input's up/down recall.
-- Capped at the last 100 entries per instance by the writer.
CREATE TABLE command_history (
    id          INTEGER PRIMARY KEY,
    instance_id INTEGER NOT NULL REFERENCES instances(id) ON DELETE CASCADE,
    command     TEXT    NOT NULL,
    ran_at      TEXT    NOT NULL
);

CREATE INDEX idx_command_history_instance ON command_history(instance_id, id);
