CREATE TABLE registry_change_events (
    event_id    INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type  INTEGER NOT NULL DEFAULT 0,
    environment_id TEXT,
    deployment_revision_id INTEGER,
    account_id  TEXT,
    grantee_account_id TEXT,
    domains     TEXT,
    changed_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_registry_change_events_id ON registry_change_events (event_id);
