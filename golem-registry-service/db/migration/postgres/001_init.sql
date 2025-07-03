CREATE TABLE plans
(
    plan_id UUID NOT NULL,
    name    TEXT NOT NULL,
    CONSTRAINT plans_pk
        PRIMARY KEY (plan_id)
);

CREATE TABLE usage_types
(
    usage_type INT  NOT NULL,
    name       TEXT NOT NULL,
    CONSTRAINT plan_usage_types_pk
        PRIMARY KEY (usage_type)
);

INSERT INTO usage_types (usage_type, name)
VALUES (0, 'TOTAL_APP_COUNT'),
       (1, 'TOTAL_ENV_COUNT'),
       (2, 'TOTAL_COMPONENT_COUNT'),
       (3, 'TOTAL_WORKER_COUNT'),
       (4, 'TOTAL_COMPONENT_STORAGE_BYTES'),
       (5, 'MONTHLY_GAS_LIMIT'),
       (6, 'MONTHLY_COMPONENT_UPLOAD_LIMIT_BYTES');

CREATE TABLE plan_usage_limits
(
    plan_id    UUID   NOT NULL,
    usage_type INT    NOT NULL,
    usage_key  TEXT   NOT NULL,
    value      BIGINT NOT NULL,
    CONSTRAINT plan_usage_limits_pk
        PRIMARY KEY (plan_id, usage_type, usage_key),
    CONSTRAINT plan_usage_limits_plans
        FOREIGN KEY (plan_id) REFERENCES plans,
    CONSTRAINT plan_usage_limits_usage_types
        FOREIGN KEY (usage_type) REFERENCES usage_types
);

CREATE INDEX plan_usage_limits_usage_type_idx
    ON plan_usage_limits (usage_type);

CREATE TABLE accounts
(
    account_id  UUID      NOT NULL,
    email       TEXT      NOT NULL,

    created_at  TIMESTAMP NOT NULL,
    updated_at  TIMESTAMP NOT NULL,
    deleted_at  TIMESTAMP,
    modified_by UUID      NOT NULL,

    name        TEXT      NOT NULL,
    plan_id     UUID      NOT NULL,

    CONSTRAINT accounts_pk
        PRIMARY KEY (account_id),
    CONSTRAINT accounts_plans_fk
        FOREIGN KEY (plan_id) REFERENCES plans
);

CREATE UNIQUE INDEX accounts_email_uk
    ON accounts (email)
    WHERE deleted_at IS NULL;

CREATE INDEX accounts_plan_id_idx ON accounts (plan_id);

CREATE TABLE account_revisions
(
    account_id  UUID      NOT NULL,
    revision_id BIGINT    NOT NULL,
    email       TEXT      NOT NULL,

    created_at  TIMESTAMP NOT NULL,
    deleted     BOOLEAN   NOT NULL,

    plan_id     UUID      NOT NULL,
    name        TEXT      NOT NULL,

    CONSTRAINT account_revisions_pk
        PRIMARY KEY (account_id, revision_id),
    CONSTRAINT account_revisions_fk
        FOREIGN KEY (account_id) REFERENCES accounts
);

CREATE TABLE tokens
(
    token_id   UUID      NOT NULL,
    secret     UUID      NOT NULL,
    account_id UUID      NOT NULL,
    created_at TIMESTAMP NOT NULL,
    expires_at TIMESTAMP NOT NULL,
    CONSTRAINT tokens_pk
        PRIMARY KEY (token_id),
    CONSTRAINT tokens_secret_uk
        UNIQUE (secret),
    CONSTRAINT tokens_account_fk
        FOREIGN KEY (account_id)
            REFERENCES accounts
);

CREATE INDEX tokens_account
    ON tokens (account_id);

CREATE TABLE oauth2_tokens
(
    provider    TEXT NOT NULL,
    external_id TEXT NOT NULL,
    token_id    UUID NOT NULL,
    account_id  UUID NOT NULL,
    CONSTRAINT oauth2_tokens_pk PRIMARY KEY (provider, external_id),
    CONSTRAINT oauth2_tokens_token_fk FOREIGN KEY (token_id) REFERENCES tokens,
    CONSTRAINT oauth2_tokens_account_fk FOREIGN KEY (account_id) REFERENCES accounts
);

CREATE INDEX oauth2_tokens_token_idx
    ON oauth2_tokens (token_id);

CREATE INDEX oauth2_tokens_account_idx
    ON oauth2_tokens (account_id);

CREATE TABLE oauth2_web_flow_state
(
    oauth2_state TEXT      NOT NULL,
    metadata     BYTEA     NOT NULL,
    token_id     UUID      NOT NULL,
    created_at   TIMESTAMP NOT NULL,
    CONSTRAINT oauth2_web_flow_state_pk PRIMARY KEY (oauth2_state),
    CONSTRAINT oauth2_web_flow_state_token_fk FOREIGN KEY (token_id) REFERENCES tokens
);

CREATE INDEX oauth2_web_flow_state_token_idx
    ON oauth2_web_flow_state (token_id);

CREATE TABLE account_creation_attempts
(
    oauth2_provider TEXT      NOT NULL,
    external_id     TEXT      NOT NULL,
    name            TEXT      NOT NULL,
    email           TEXT      NOT NULL,
    all_emails      JSONB     NOT NULL,
    first_attempt   TIMESTAMP NOT NULL,
    latest_attempt  TIMESTAMP NOT NULL,
    attempts_count  INTEGER   NOT NULL,
    PRIMARY KEY (oauth2_provider, external_id)
);


CREATE TABLE account_usage_stats
(
    account_id UUID   NOT NULL,
    usage_type INT    NOT NULL,
    usage_key  TEXT   NOT NULL,
    value      BIGINT NOT NULL,
    CONSTRAINT account_usage_stats_pk
        PRIMARY KEY (account_id, usage_type, usage_key),
    CONSTRAINT account_usage_stats_accounts_fk
        FOREIGN KEY (account_id) REFERENCES accounts,
    CONSTRAINT plan_usage_limits_usage_types
        FOREIGN KEY (usage_type) REFERENCES usage_types
);

CREATE INDEX account_usage_stats_usage_type_idx ON account_usage_stats (usage_type);

CREATE TABLE applications
(
    application_id UUID      NOT NULL,
    name           TEXT      NOT NULL,
    account_id     UUID      NOT NULL,

    created_at     TIMESTAMP NOT NULL,
    updated_at     TIMESTAMP NOT NULL,
    deleted_at     TIMESTAMP,
    modified_by    UUID      NOT NULL,

    CONSTRAINT applications_pk
        PRIMARY KEY (application_id),
    CONSTRAINT applications_accounts_fk
        FOREIGN KEY (account_id) REFERENCES accounts
);

CREATE UNIQUE INDEX applications_name_uk
    ON applications (account_id, name)
    WHERE deleted_at IS NULL;

CREATE TABLE application_revisions
(
    application_id UUID      NOT NULL,
    revision_id    BIGINT    NOT NULL,
    name           TEXT      NOT NULL,
    account_id     UUID      NOT NULL,

    created_at     TIMESTAMP NOT NULL,
    created_by     UUID      NOT NULL,
    deleted        BOOLEAN   NOT NULL,

    CONSTRAINT application_revisions_pk
        PRIMARY KEY (application_id, revision_id),
    CONSTRAINT application_revisions_applications_fk
        FOREIGN KEY (application_id) REFERENCES applications
);

CREATE INDEX application_revisions_name_idx ON application_revisions (application_id, name);

CREATE TABLE environments
(
    environment_id      UUID      NOT NULL,
    name                TEXT      NOT NULL,
    application_id      UUID      NOT NULL,

    created_at          TIMESTAMP NOT NULL,
    updated_at          TIMESTAMP NOT NULL,
    deleted_at          TIMESTAMP,
    modified_by         UUID      NOT NULL,

    current_revision_id BIGINT    NOT NULL,

    CONSTRAINT environments_pk
        PRIMARY KEY (environment_id),
    CONSTRAINT environments_applications_fk
        FOREIGN KEY (application_id) REFERENCES applications
);

CREATE UNIQUE INDEX environments_app_name_uk
    ON environments (application_id, name)
    WHERE deleted_at IS NULL;

CREATE TABLE environment_revisions
(
    environment_id      UUID      NOT NULL,
    revision_id         BIGINT    NOT NULL,

    created_at          TIMESTAMP NOT NULL,
    created_by          UUID      NOT NULL,
    deleted             BOOLEAN   NOT NULL,

    compatibility_check BOOL      NOT NULL,
    version_check       BOOL      NOT NULL,
    security_overrides  BOOL      NOT NULL,
    hash                BYTEA     NOT NULL,

    CONSTRAINT environment_revisions_pk
        PRIMARY KEY (environment_id, revision_id),
    CONSTRAINT environment_revisions_fk
        FOREIGN KEY (environment_id) REFERENCES environments
);

CREATE INDEX environment_revisions_latest_idx
    ON environment_revisions (environment_id, revision_id DESC);

CREATE TABLE component_revisions
(
    environment_id               UUID      NOT NULL,
    component_id                 UUID      NOT NULL,
    name                         TEXT      NOT NULL,
    revision_id                  BIGINT    NOT NULL,
    version                      TEXT      NOT NULL,
    created_at                   TIMESTAMP NOT NULL,
    created_by                   UUID      NOT NULL,
    component_type               INTEGER   NOT NULL,
    size                         INTEGER   NOT NULL,
    metadata                     BYTEA     NOT NULL,
    env                          JSONB,
    status                       INTEGER   NOT NULL,
    object_store_key             TEXT,
    transformed_object_store_key TEXT,
    hash                         BYTEA     NOT NULL,
    CONSTRAINT component_revisions_pk
        PRIMARY KEY (component_id, revision_id),
    CONSTRAINT component_revisions_environments_fk
        FOREIGN KEY (environment_id) REFERENCES environments,
    CONSTRAINT component_revisions_id_uk
        UNIQUE (environment_id, component_id, revision_id),
    CONSTRAINT component_revisions_name_uk
        UNIQUE (environment_id, component_id, revision_id, name)
);

CREATE INDEX component_revisions_latest_revision_by_id_idx
    ON component_revisions (environment_id, component_id, revision_id DESC);

CREATE INDEX component_revisions_latest_revision_by_name_idx
    ON component_revisions (environment_id, name, revision_id DESC);

CREATE TABLE component_files
(
    component_id     UUID      NOT NULL,
    revision_id      BIGINT    NOT NULL,
    file_path        TEXT      NOT NULL,
    created_at       TIMESTAMP NOT NULL,
    created_by       UUID      NOT NULL,
    file_key         TEXT      NOT NULL,
    file_permissions TEXT      NOT NULL,
    hash             BYTEA     NOT NULL,
    CONSTRAINT component_files_pk
        PRIMARY KEY (component_id, revision_id, file_path),
    CONSTRAINT component_files_components_fk
        FOREIGN KEY (component_id, revision_id) REFERENCES component_revisions
);

CREATE TABLE deployment_revisions
(
    environment_id  UUID      NOT NULL,
    revision_id     BIGINT    NOT NULL,
    version         TEXT      NOT NULL,
    created_at      TIMESTAMP NOT NULL,
    created_by      UUID      NOT NULL,
    deployment_kind INTEGER   NOT NULL,
    hash            BYTEA     NOT NULL,
    CONSTRAINT deployments_revisions_pk
        PRIMARY KEY (environment_id, revision_id),
    CONSTRAINT deployments_revisions_environments_fk
        FOREIGN KEY (environment_id) REFERENCES environments
);

CREATE TABLE current_deployment_revisions
(
    environment_id      UUID      NOT NULL,
    revision_id         BIGINT    NOT NULL,
    created_at          TIMESTAMP NOT NULL,
    created_by          UUID      NOT NULL,
    current_revision_id BIGINT    NOT NULL,
    CONSTRAINT current_deployment_revisions_pk
        PRIMARY KEY (environment_id, revision_id),
    CONSTRAINT current_deployment_revisions_deployment_revisions_fk
        FOREIGN KEY (environment_id, current_revision_id) REFERENCES deployment_revisions (environment_id, revision_id)
);

CREATE INDEX current_deployment_revisions_current_idx
    ON current_deployment_revisions (environment_id, current_revision_id);

CREATE INDEX current_deployment_revisions_latest_idx
    ON current_deployment_revisions (environment_id, revision_id DESC);

CREATE TABLE current_deployments
(
    environment_id      UUID   NOT NULL,
    current_revision_id BIGINT NOT NULL,
    CONSTRAINT current_deployments_pk
        PRIMARY KEY (environment_id, current_revision_id),
    CONSTRAINT current_deployments_deployments_revisions_fk
        FOREIGN KEY (environment_id, current_revision_id) REFERENCES deployment_revisions (environment_id, revision_id)
);

CREATE TABLE deployment_component_revisions
(
    environment_id         UUID   NOT NULL,
    deployment_revision_id BIGINT NOT NULL,
    component_id           UUID   NOT NULL,
    component_revision_id  BIGINT NOT NULL,
    CONSTRAINT deployment_component_revisions_pk
        PRIMARY KEY (environment_id, deployment_revision_id, component_id, component_revision_id),
    CONSTRAINT deployment_component_revisions_deployment_fk
        FOREIGN KEY (environment_id, deployment_revision_id)
            REFERENCES deployment_revisions (environment_id, revision_id),
    CONSTRAINT deployment_component_revisions_component_fk
        FOREIGN KEY (environment_id, component_id, component_revision_id)
            REFERENCES component_revisions (environment_id, component_id, revision_id)
);

CREATE INDEX deployment_component_revisions_component_idx
    ON deployment_component_revisions (environment_id, component_id, component_revision_id);
