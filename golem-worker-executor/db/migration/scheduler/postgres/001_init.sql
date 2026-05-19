CREATE TABLE scheduled_actions (
    schedule_id     UUID NOT NULL,
    due_at_ms       BIGINT NOT NULL,
    available_at_ms BIGINT NOT NULL,
    shard_id        BIGINT NOT NULL,
    action          BYTEA NOT NULL,
    lease_owner     UUID NULL,
    lease_until_ms  BIGINT NULL,
    attempt_count   INTEGER NOT NULL DEFAULT 0,
    CONSTRAINT scheduled_actions_pk PRIMARY KEY (schedule_id)
);

CREATE INDEX scheduled_actions_claim_idx
    ON scheduled_actions (shard_id, available_at_ms, schedule_id);

ALTER TABLE scheduled_actions SET (
    autovacuum_vacuum_scale_factor = 0.01,
    autovacuum_vacuum_threshold = 1024,
    autovacuum_analyze_scale_factor = 0.02,
    autovacuum_analyze_threshold = 1024,
    autovacuum_vacuum_cost_limit = 2000
);
