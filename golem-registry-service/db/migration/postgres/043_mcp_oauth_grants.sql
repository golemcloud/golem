CREATE UNIQUE INDEX security_schemes_id_environment_uk
    ON security_schemes (security_scheme_id, environment_id);

CREATE TABLE mcp_oauth_grants (
    environment_id UUID NOT NULL,
    security_scheme_id UUID NOT NULL,
    security_scheme_revision BIGINT NOT NULL,
    credential_owner_account_id UUID NOT NULL,
    resource_url TEXT NOT NULL,
    generation UUID NOT NULL,
    status TEXT NOT NULL,
    state_hash BYTEA,
    consent_expires_at TIMESTAMP,
    flow_secrets BYTEA,
    token_secrets BYTEA,
    CONSTRAINT mcp_oauth_grants_pk PRIMARY KEY
        (environment_id, security_scheme_id, security_scheme_revision, credential_owner_account_id, resource_url),
    CONSTRAINT mcp_oauth_grants_environment_fk FOREIGN KEY (environment_id) REFERENCES environments,
    CONSTRAINT mcp_oauth_grants_owner_fk FOREIGN KEY (credential_owner_account_id) REFERENCES accounts,
    CONSTRAINT mcp_oauth_grants_scheme_environment_fk FOREIGN KEY (security_scheme_id, environment_id)
        REFERENCES security_schemes (security_scheme_id, environment_id),
    CONSTRAINT mcp_oauth_grants_scheme_revision_fk FOREIGN KEY (security_scheme_id, security_scheme_revision)
        REFERENCES security_scheme_revisions (security_scheme_id, revision_id),
    CONSTRAINT mcp_oauth_grants_status_ck CHECK
        (status IN ('pending-consent', 'exchanging', 'granted', 'refreshing', 'reauthorization-required', 'revoked'))
);

CREATE UNIQUE INDEX mcp_oauth_grants_state_hash_uk ON mcp_oauth_grants (state_hash) WHERE state_hash IS NOT NULL;
