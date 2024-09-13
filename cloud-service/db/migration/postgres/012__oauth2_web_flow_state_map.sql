CREATE TABLE IF NOT EXISTS oauth2_web_flow_state (
    oauth2_state TEXT PRIMARY KEY,
    metadata     BYTEA NOT NULL,
    -- NULL token_id indicates pending OAuth callback linkage
    token_id     UUID REFERENCES tokens(id),
    created_at   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);