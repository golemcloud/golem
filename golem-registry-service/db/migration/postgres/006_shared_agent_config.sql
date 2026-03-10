CREATE TABLE agent_secrets
(
    agent_secret_id        UUID      NOT NULL,

    environment_id   UUID      NOT NULL,
    -- string containing the path array as json
    path             JSONB    NOT NULL,

    agent_secret_data BYTEA   NOT NULL,

    created_at          TIMESTAMP NOT NULL,
    updated_at          TIMESTAMP NOT NULL,
    deleted_at          TIMESTAMP,
    modified_by         UUID      NOT NULL,

    current_revision_id BIGINT    NOT NULL,

    CONSTRAINT agent_secrets_pk
        PRIMARY KEY (agent_secret_id),
    CONSTRAINT agent_secrets_environments_fk
        FOREIGN KEY (environment_id) REFERENCES environments
);

CREATE UNIQUE INDEX agent_secrets_environment_path_uk ON agent_secrets (environment_id, path)
    WHERE deleted_at IS NULL;

CREATE TABLE agent_secret_revisions
(
    agent_secret_id                 UUID      NOT NULL,
    revision_id                  BIGINT    NOT NULL,

    agent_secret_revision_data BYTEA   NOT NULL,

    created_at           TIMESTAMP NOT NULL,
    created_by           UUID      NOT NULL,
    deleted              BOOLEAN   NOT NULL,

    CONSTRAINT agent_secret_revisions_pk
        PRIMARY KEY (agent_secret_id, revision_id),
    CONSTRAINT agent_secret_revisions_agent_secrets_fk
        FOREIGN KEY (agent_secret_id) REFERENCES agent_secrets
);
