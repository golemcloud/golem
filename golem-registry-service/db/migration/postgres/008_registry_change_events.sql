CREATE TABLE registry_change_events (
    event_id    BIGSERIAL PRIMARY KEY,
    event_type  SMALLINT NOT NULL DEFAULT 0,
    environment_id UUID,
    deployment_revision_id BIGINT,
    account_id  UUID,
    grantee_account_id UUID,
    domains     TEXT[],
    changed_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_registry_change_events_id ON registry_change_events (event_id);
