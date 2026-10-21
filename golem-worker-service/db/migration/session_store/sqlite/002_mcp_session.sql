CREATE TABLE mcp_session
(
    session_id UUID   NOT NULL,
    value      BYTEA  NOT NULL,
    expires_at BIGINT NOT NULL,
    CONSTRAINT mcp_session_pk
        PRIMARY KEY (session_id)
);

CREATE INDEX mcp_session_expires_at_idx ON mcp_session (expires_at);
