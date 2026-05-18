CREATE TABLE scheduled_actions (
    schedule_id     TEXT NOT NULL,
    due_at_ms       INTEGER NOT NULL,
    available_at_ms INTEGER NOT NULL,
    shard_id        INTEGER NOT NULL,
    action          BLOB NOT NULL,
    lease_owner     TEXT NULL,
    lease_until_ms  INTEGER NULL,
    attempt_count   INTEGER NOT NULL DEFAULT 0,
    CONSTRAINT scheduled_actions_pk PRIMARY KEY (schedule_id)
);

CREATE INDEX scheduled_actions_claim_idx
    ON scheduled_actions (shard_id, available_at_ms, schedule_id);
