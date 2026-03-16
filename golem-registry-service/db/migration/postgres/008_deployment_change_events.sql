CREATE TABLE deployment_change_events (
    event_id    BIGSERIAL PRIMARY KEY,
    environment_id UUID NOT NULL,
    deployment_revision_id BIGINT NOT NULL,
    changed_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_deployment_change_events_id ON deployment_change_events (event_id);
