CREATE TABLE oidc_pending_login
(
    state      TEXT   NOT NULL,
    value      BYTEA  NOT NULL,
    expires_at BIGINT NOT NULL,
    CONSTRAINT oidc_pending_login_pk
        PRIMARY KEY (state)
);

CREATE INDEX oidc_pending_login_expires_at_idx ON oidc_pending_login (expires_at);

CREATE TABLE oidc_session
(
    session_id UUID   NOT NULL,
    value      BYTEA  NOT NULL,
    expires_at BIGINT NOT NULL,
    CONSTRAINT oidc_session_pk
        PRIMARY KEY (session_id)
);

CREATE INDEX oidc_session_expires_at_idx ON oidc_session (expires_at);

CREATE TABLE oidc_pkce_pending
(
    state_digest TEXT   NOT NULL,
    value        BYTEA  NOT NULL,
    expires_at   BIGINT NOT NULL,
    CONSTRAINT oidc_pkce_pending_pk
        PRIMARY KEY (state_digest)
);

CREATE INDEX oidc_pkce_pending_expires_at_idx ON oidc_pkce_pending (expires_at);

CREATE TABLE oidc_pkce_authorization_code
(
    code_digest TEXT   NOT NULL,
    value       BYTEA  NOT NULL,
    expires_at  BIGINT NOT NULL,
    CONSTRAINT oidc_pkce_authorization_code_pk
        PRIMARY KEY (code_digest)
);

CREATE INDEX oidc_pkce_authorization_code_expires_at_idx ON oidc_pkce_authorization_code (expires_at);

CREATE TABLE oidc_pkce_bearer
(
    token_digest TEXT   NOT NULL,
    value        BYTEA  NOT NULL,
    expires_at   BIGINT NOT NULL,
    CONSTRAINT oidc_pkce_bearer_pk
        PRIMARY KEY (token_digest)
);

CREATE INDEX oidc_pkce_bearer_expires_at_idx ON oidc_pkce_bearer (expires_at);

CREATE TABLE mcp_pending_auth
(
    state      TEXT   NOT NULL,
    value      BYTEA  NOT NULL,
    expires_at BIGINT NOT NULL,
    CONSTRAINT mcp_pending_auth_pk
        PRIMARY KEY (state)
);

CREATE INDEX mcp_pending_auth_expires_at_idx ON mcp_pending_auth (expires_at);

CREATE TABLE mcp_proxy_code
(
    code       TEXT   NOT NULL,
    value      BYTEA  NOT NULL,
    expires_at BIGINT NOT NULL,
    CONSTRAINT mcp_proxy_code_pk
        PRIMARY KEY (code)
);

CREATE INDEX mcp_proxy_code_expires_at_idx ON mcp_proxy_code (expires_at);
