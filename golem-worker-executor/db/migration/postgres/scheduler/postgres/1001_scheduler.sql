CREATE TABLE scheduled_actions (
    schedule_id    UUID NOT NULL,
    due_at_ms      BIGINT NOT NULL,
    routing_hash   BIGINT NOT NULL,
    action         BYTEA NOT NULL,
    lease_owner    UUID NULL,
    lease_until_ms BIGINT NULL,
    attempt_count  INTEGER NOT NULL DEFAULT 0,
    CONSTRAINT scheduled_actions_pk PRIMARY KEY (schedule_id)
);

CREATE INDEX scheduled_actions_due_idx
    ON scheduled_actions (due_at_ms, schedule_id);

CREATE INDEX scheduled_actions_due_lease_idx
    ON scheduled_actions (due_at_ms, lease_until_ms, schedule_id);
