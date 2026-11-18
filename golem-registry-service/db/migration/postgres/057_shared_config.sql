CREATE TABLE shared_configs
(
    shared_config_id    UUID      NOT NULL,
    environment_id      UUID      NOT NULL,
    path                JSONB     NOT NULL,
    schema              BYTEA     NOT NULL,
    current_revision_id BIGINT    NOT NULL,
    created_at          TIMESTAMP NOT NULL,
    updated_at          TIMESTAMP NOT NULL,
    deleted_at          TIMESTAMP,
    modified_by         UUID      NOT NULL,

    CONSTRAINT shared_configs_pk PRIMARY KEY (shared_config_id),
    CONSTRAINT shared_configs_environments_fk
        FOREIGN KEY (environment_id) REFERENCES environments
);

CREATE UNIQUE INDEX shared_configs_environment_path_uk ON shared_configs (environment_id, path)
    WHERE deleted_at IS NULL;

CREATE TABLE shared_config_revisions
(
    shared_config_id UUID      NOT NULL,
    revision_id      BIGINT    NOT NULL,
    revision_data    BYTEA     NOT NULL,
    created_at       TIMESTAMP NOT NULL,
    created_by       UUID      NOT NULL,
    deleted          BOOLEAN   NOT NULL,

    CONSTRAINT shared_config_revisions_pk PRIMARY KEY (shared_config_id, revision_id),
    CONSTRAINT shared_config_revisions_shared_configs_fk
        FOREIGN KEY (shared_config_id) REFERENCES shared_configs
);
