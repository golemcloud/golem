CREATE TABLE deployment_change_events (
    event_id    INTEGER PRIMARY KEY AUTOINCREMENT,
    environment_id TEXT NOT NULL,
    deployment_revision_id INTEGER NOT NULL,
    changed_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_deployment_change_events_id ON deployment_change_events (event_id);
