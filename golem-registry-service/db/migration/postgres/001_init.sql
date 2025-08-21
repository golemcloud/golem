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
    value      BIGINT NOT NULL,
    CONSTRAINT plan_usage_limits_pk
        PRIMARY KEY (plan_id, usage_type),
    CONSTRAINT plan_usage_limits_plans
        FOREIGN KEY (plan_id) REFERENCES plans,
    CONSTRAINT plan_usage_limits_usage_types
        FOREIGN KEY (usage_type) REFERENCES usage_types
);

CREATE INDEX plan_usage_limits_usage_type_idx
    ON plan_usage_limits (usage_type);

CREATE TABLE accounts
(
    account_id          UUID      NOT NULL,
    email               TEXT      NOT NULL,

    created_at          TIMESTAMP NOT NULL,
    updated_at          TIMESTAMP NOT NULL,
    deleted_at          TIMESTAMP,
    modified_by         UUID      NOT NULL,

    current_revision_id BIGINT    NOT NULL,

    CONSTRAINT accounts_pk
        PRIMARY KEY (account_id)
);

CREATE UNIQUE INDEX accounts_email_uk
    ON accounts (email)
    WHERE deleted_at IS NULL;

CREATE TABLE account_revisions
(
    account_id  UUID      NOT NULL,
    revision_id BIGINT    NOT NULL,

    name        TEXT      NOT NULL,
    email       TEXT      NOT NULL,
    plan_id     UUID      NOT NULL,

    created_at  TIMESTAMP NOT NULL,
    created_by  UUID      NOT NULL,
    deleted     BOOLEAN   NOT NULL,

    CONSTRAINT account_revisions_pk
        PRIMARY KEY (account_id, revision_id),
    CONSTRAINT account_revisions_accounts_fk
        FOREIGN KEY (account_id) REFERENCES accounts,
    CONSTRAINT account_revisions_plans_fk
        FOREIGN KEY (plan_id) REFERENCES plans
);

CREATE TABLE account_revision_roles
(
    account_id  UUID   NOT NULL,
    revision_id BIGINT NOT NULL,
    role        INT    NOT NULL,

    CONSTRAINT account_revision_roles_pk
        PRIMARY KEY (account_id, revision_id, role),
    CONSTRAINT account_revision_roles_account_revisions_fk
        FOREIGN KEY (account_id, revision_id) REFERENCES account_revisions
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
    token_id    UUID,
    account_id  UUID NOT NULL,
    CONSTRAINT oauth2_tokens_pk PRIMARY KEY (provider, external_id),
    CONSTRAINT oauth2_tokens_token_fk FOREIGN KEY (token_id) REFERENCES tokens,
    CONSTRAINT oauth2_tokens_account_fk FOREIGN KEY (account_id) REFERENCES accounts
);

CREATE UNIQUE INDEX oauth2_tokens_token_idx
    ON oauth2_tokens (token_id);

CREATE INDEX oauth2_tokens_account_idx
    ON oauth2_tokens (account_id);

CREATE TABLE oauth2_web_flow_states
(
    state_id   UUID      NOT NULL,
    metadata   BYTEA     NOT NULL,
    token_id   UUID      NULL,
    created_at TIMESTAMP NOT NULL,
    CONSTRAINT oauth2_web_flow_states_pk PRIMARY KEY (state_id),
    CONSTRAINT oauth2_web_flow_states_token_fk FOREIGN KEY (token_id) REFERENCES tokens
);

CREATE INDEX oauth2_web_flow_states_token_idx
    ON oauth2_web_flow_states (token_id);

CREATE TABLE account_usage_stats
(
    account_id UUID      NOT NULL,
    usage_type INT       NOT NULL,
    usage_key  TEXT      NOT NULL,
    value      BIGINT    NOT NULL,
    updated_at TIMESTAMP NOT NULL,
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

    hash                BYTEA     NOT NULL,

    created_at          TIMESTAMP NOT NULL,
    created_by          UUID      NOT NULL,
    deleted             BOOLEAN   NOT NULL,

    compatibility_check BOOL      NOT NULL,
    version_check       BOOL      NOT NULL,
    security_overrides  BOOL      NOT NULL,

    CONSTRAINT environment_revisions_pk
        PRIMARY KEY (environment_id, revision_id),
    CONSTRAINT environment_revisions_fk
        FOREIGN KEY (environment_id) REFERENCES environments
);

CREATE INDEX environment_revisions_latest_idx
    ON environment_revisions (environment_id, revision_id DESC);

CREATE TABLE components
(
    component_id        UUID      NOT NULL,
    name                TEXT      NOT NULL,
    environment_id      UUID      NOT NULL,

    created_at          TIMESTAMP NOT NULL,
    updated_at          TIMESTAMP NOT NULL,
    deleted_at          TIMESTAMP,
    modified_by         UUID      NOT NULL,

    current_revision_id BIGINT    NOT NULL,

    CONSTRAINT components_pk
        PRIMARY KEY (component_id),
    CONSTRAINT components_environments_fk
        FOREIGN KEY (environment_id) REFERENCES environments
);

CREATE UNIQUE INDEX components_name_uk
    ON components (environment_id, name)
    WHERE deleted_at IS NULL;

CREATE TABLE component_revisions
(
    component_id                 UUID      NOT NULL,
    revision_id                  BIGINT    NOT NULL,
    version                      TEXT      NOT NULL,

    hash                         BYTEA     NOT NULL,

    created_at                   TIMESTAMP NOT NULL,
    created_by                   UUID      NOT NULL,
    deleted                      BOOLEAN   NOT NULL,

    component_type               INTEGER   NOT NULL,
    size                         INTEGER   NOT NULL,
    metadata                     BYTEA     NOT NULL,
    original_env                 JSONB     NOT NULL,
    env                          JSONB     NOT NULL,
    object_store_key             TEXT      NOT NULL,
    binary_hash                  BYTEA     NOT NULL,
    transformed_object_store_key TEXT      NOT NULL,

    CONSTRAINT component_revisions_pk
        PRIMARY KEY (component_id, revision_id),
    CONSTRAINT component_revisions_components_fk
        FOREIGN KEY (component_id) REFERENCES components
);

CREATE INDEX component_revisions_latest_revision_by_id_idx
    ON component_revisions (component_id, revision_id DESC);

CREATE TABLE original_component_files
(
    component_id     UUID      NOT NULL,
    revision_id      BIGINT    NOT NULL,
    file_path        TEXT      NOT NULL,

    hash             BYTEA     NOT NULL,

    created_at       TIMESTAMP NOT NULL,
    created_by       UUID      NOT NULL,

    file_key         TEXT      NOT NULL,
    file_permissions TEXT      NOT NULL,

    CONSTRAINT original_component_files_pk
        PRIMARY KEY (component_id, revision_id, file_path),
    CONSTRAINT original_component_files_components_fk
        FOREIGN KEY (component_id, revision_id) REFERENCES component_revisions
);

CREATE TABLE component_files
(
    component_id     UUID      NOT NULL,
    revision_id      BIGINT    NOT NULL,
    file_path        TEXT      NOT NULL,

    hash             BYTEA     NOT NULL,

    created_at       TIMESTAMP NOT NULL,
    created_by       UUID      NOT NULL,

    file_key         TEXT      NOT NULL,
    file_permissions TEXT      NOT NULL,

    CONSTRAINT component_files_pk
        PRIMARY KEY (component_id, revision_id, file_path),
    CONSTRAINT component_files_components_fk
        FOREIGN KEY (component_id, revision_id) REFERENCES component_revisions
);

CREATE TABLE deployment_revisions
(
    environment_id UUID      NOT NULL,
    revision_id    BIGINT    NOT NULL,
    version        TEXT      NOT NULL,

    hash           BYTEA     NOT NULL,

    created_at     TIMESTAMP NOT NULL,
    created_by     UUID      NOT NULL,

    CONSTRAINT deployment_revisions_pk
        PRIMARY KEY (environment_id, revision_id),
    CONSTRAINT deployment_revisions_environments_fk
        FOREIGN KEY (environment_id) REFERENCES environments
);

CREATE INDEX deployment_revisions_latest_idx
    ON deployment_revisions (environment_id, revision_id DESC);

CREATE TABLE current_deployment_revisions
(
    environment_id         UUID      NOT NULL,
    revision_id            BIGINT    NOT NULL,
    created_at             TIMESTAMP NOT NULL,
    created_by             UUID      NOT NULL,
    deployment_revision_id BIGINT    NOT NULL,
    deployment_version     TEXT      NOT NULL,
    CONSTRAINT current_deployment_revisions_pk
        PRIMARY KEY (environment_id, revision_id),
    CONSTRAINT current_deployment_revisions_deployment_revisions_fk
        FOREIGN KEY (environment_id, deployment_revision_id) REFERENCES deployment_revisions (environment_id, revision_id)
);

CREATE INDEX current_deployment_revisions_current_idx
    ON current_deployment_revisions (environment_id, deployment_revision_id);

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
        FOREIGN KEY (component_id, component_revision_id)
            REFERENCES component_revisions (component_id, revision_id)
);

CREATE INDEX deployment_component_revisions_component_idx
    ON deployment_component_revisions (component_id, component_revision_id);

CREATE TABLE http_api_definitions
(
    http_api_definition_id UUID      NOT NULL,
    name                   TEXT      NOT NULL,
    environment_id         UUID      NOT NULL,

    created_at             TIMESTAMP NOT NULL,
    updated_at             TIMESTAMP NOT NULL,
    deleted_at             TIMESTAMP,
    modified_by            UUID      NOT NULL,

    current_revision_id    BIGINT    NOT NULL,

    CONSTRAINT http_api_definitions_pk
        PRIMARY KEY (http_api_definition_id),
    CONSTRAINT http_api_definitions_fk
        FOREIGN KEY (environment_id) REFERENCES environments
);

CREATE UNIQUE INDEX http_api_definitions_name_uk
    ON http_api_definitions (environment_id, name)
    WHERE deleted_at IS NULL;

CREATE TABLE http_api_definition_revisions
(
    http_api_definition_id UUID      NOT NULL,
    revision_id            BIGINT    NOT NULL,
    version                TEXT      NOT NULL,

    hash                   BYTEA     NOT NULL,

    created_at             TIMESTAMP NOT NULL,
    created_by             UUID      NOT NULL,
    deleted                BOOLEAN   NOT NULL,

    definition             BYTEA     NOT NULL,

    CONSTRAINT http_api_definition_revisions_pk
        PRIMARY KEY (http_api_definition_id, revision_id),
    CONSTRAINT http_api_definition_revisions_http_api_definitions_fk
        FOREIGN KEY (http_api_definition_id) REFERENCES http_api_definitions
);

CREATE INDEX http_api_definitions_revisions_latest_revision_by_id_idx
    ON http_api_definition_revisions (http_api_definition_id, revision_id DESC);

CREATE TABLE http_api_deployments
(
    http_api_deployment_id UUID      NOT NULL,
    environment_id         UUID      NOT NULL,

    host                   TEXT      NOT NULL,
    subdomain              TEXT,

    created_at             TIMESTAMP NOT NULL,
    updated_at             TIMESTAMP NOT NULL,
    deleted_at             TIMESTAMP,
    modified_by            UUID      NOT NULL,

    current_revision_id    BIGINT    NOT NULL,

    CONSTRAINT http_api_deployments_pk
        PRIMARY KEY (http_api_deployment_id),
    CONSTRAINT http_api_deployments_environments_fk
        FOREIGN KEY (environment_id) REFERENCES environments
);

CREATE UNIQUE INDEX http_api_deployments_name_uk
    ON components (environment_id, name)
    WHERE deleted_at IS NULL;

CREATE TABLE http_api_deployment_revisions
(
    http_api_deployment_id UUID      NOT NULL,
    revision_id            BIGINT    NOT NULL,

    hash                   BYTEA     NOT NULL,

    created_at             TIMESTAMP NOT NULL,
    created_by             UUID      NOT NULL,
    deleted                BOOLEAN   NOT NULL,

    CONSTRAINT http_api_deployment_revisions_pk
        PRIMARY KEY (http_api_deployment_id, revision_id),
    CONSTRAINT http_api_deployment_revisions_deployments_fk
        FOREIGN KEY (http_api_deployment_id) REFERENCES http_api_deployments
);

CREATE INDEX http_api_deployment_revisions_latest_revision_by_id_idx
    ON http_api_deployment_revisions (http_api_deployment_id, revision_id DESC);

CREATE TABLE http_api_deployment_definitions
(
    http_api_deployment_id UUID   NOT NULL,
    revision_id            BIGINT NOT NULL,
    http_definition_id     UUID   NOT NULL,

    CONSTRAINT http_api_deployment_definitions_pk
        PRIMARY KEY (http_api_deployment_id, revision_id, http_definition_id),
    CONSTRAINT http_api_deployment_definitions_http_api_deployments_fk
        FOREIGN KEY (http_api_deployment_id, revision_id) REFERENCES http_api_deployment_revisions
);

CREATE TABLE deployment_http_api_definition_revisions
(
    environment_id                  UUID   NOT NULL,
    deployment_revision_id          BIGINT NOT NULL,
    http_api_definition_id          UUID   NOT NULL,
    http_api_definition_revision_id BIGINT NOT NULL,
    CONSTRAINT deployment_http_api_definition_revisions_pk
        PRIMARY KEY (environment_id, deployment_revision_id, http_api_definition_id),
    CONSTRAINT deployment_http_api_definition_revisions_deployment_fk
        FOREIGN KEY (environment_id, deployment_revision_id)
            REFERENCES deployment_revisions (environment_id, revision_id),
    CONSTRAINT deployment_http_api_definition_revisions_http_fk
        FOREIGN KEY (http_api_definition_id, http_api_definition_revision_id)
            REFERENCES http_api_definition_revisions (http_api_definition_id, revision_id)
);

CREATE INDEX deployment_http_api_deployment_revisions_definition_idx
    ON deployment_http_api_definition_revisions (http_api_definition_id, http_api_definition_revision_id);

CREATE TABLE deployment_http_api_deployment_revisions
(
    environment_id                  UUID   NOT NULL,
    deployment_revision_id          BIGINT NOT NULL,
    http_api_deployment_id          UUID   NOT NULL,
    http_api_deployment_revision_id BIGINT NOT NULL,
    CONSTRAINT deployment_http_api_deployment_revisions_pk
        PRIMARY KEY (environment_id, deployment_revision_id, http_api_deployment_id),
    CONSTRAINT deployment_http_api_deployment_revisions_deployment_fk
        FOREIGN KEY (environment_id, deployment_revision_id)
            REFERENCES deployment_revisions (environment_id, revision_id),
    CONSTRAINT deployment_http_api_deployment_revisions_http_fk
        FOREIGN KEY (http_api_deployment_id, http_api_deployment_revision_id)
            REFERENCES http_api_deployment_revisions (http_api_deployment_id, revision_id)
);

CREATE INDEX deployment_http_api_deployment_revisions_deployment_idx
    ON deployment_http_api_deployment_revisions (http_api_deployment_id, http_api_deployment_revision_id);

CREATE TABLE plugins
(
    plugin_id             UUID      NOT NULL,
    account_id            UUID      NOT NULL,
    name                  TEXT      NOT NULL,
    version               TEXT      NOT NULL,

    created_at            TIMESTAMP NOT NULL,
    created_by            UUID      NOT NULL,
    deleted               BOOLEAN   NOT NULL,

    description           TEXT      NOT NULL,
    icon                  BYTEA     NOT NULL,
    homepage              TEXT      NOT NULL,
    plugin_type           SMALLINT  NOT NULL,
    provided_wit_package  TEXT,
    json_schema           JSONB,
    validate_url          TEXT,
    transform_url         TEXT,
    component_id          UUID,
    component_revision_id BIGINT,
    blob_storage_key      TEXT,

    CONSTRAINT plugins_pk
        PRIMARY KEY (plugin_id),
    CONSTRAINT plugins_components_fk
        FOREIGN KEY (component_id, component_revision_id)
            REFERENCES component_revisions (component_id, revision_id),
    CONSTRAINT plugins_accounts_fk
        FOREIGN KEY (account_id) REFERENCES accounts
);

CREATE UNIQUE INDEX plugins_name_version_uk ON plugins (account_id, name, version)
    WHERE deleted IS FALSE;

CREATE INDEX plugins_component_idx ON plugins (component_id, component_revision_id);

CREATE TABLE component_plugin_installations
(
    component_id UUID      NOT NULL,
    revision_id  BIGINT    NOT NULL,
    priority     INT       NOT NULL,

    created_at   TIMESTAMP NOT NULL,
    created_by   UUID      NOT NULL,

    plugin_id    UUID      NOT NULL,
    parameters   JSONB     NOT NULL,

    CONSTRAINT component_plugin_installations_pk
        PRIMARY KEY (component_id, revision_id, priority),
    CONSTRAINT component_plugin_installations_components_fk
        FOREIGN KEY (component_id, revision_id) REFERENCES component_revisions,
    CONSTRAINT component_plugin_installations_plugins_fk
        FOREIGN KEY (plugin_id) REFERENCES plugins
);

CREATE INDEX component_plugin_installations_plugin_idx ON component_plugin_installations (plugin_id);

CREATE TABLE environment_plugin_installations
(
    environment_id      UUID      NOT NULL,
    hash                BYTEA     NOT NULL,

    created_at          TIMESTAMP NOT NULL,
    updated_at          TIMESTAMP NOT NULL,
    deleted_at          TIMESTAMP,
    modified_by         UUID      NOT NULL,

    current_revision_id INT       NOT NULL,

    CONSTRAINT environment_plugin_installations_pk
        PRIMARY KEY (environment_id),
    CONSTRAINT environment_plugin_installations_environments_fk
        FOREIGN KEY (environment_id) REFERENCES environments
);

CREATE INDEX environment_plugin_installations_latest_idx
    ON environment_plugin_installations (environment_id, current_revision_id DESC);

CREATE TABLE environment_plugin_installation_revisions
(
    environment_id UUID      NOT NULL,
    revision_id    BIGINT    NOT NULL,
    priority       INT       NOT NULL,

    created_at     TIMESTAMP NOT NULL,
    created_by     UUID      NOT NULL,

    plugin_id      UUID      NOT NULL,
    parameters     JSONB     NOT NULL,

    CONSTRAINT environment_plugin_installation_revisions_pk
        PRIMARY KEY (environment_id, revision_id, priority),
    CONSTRAINT environment_plugin_installation_revisions_environment_fk
        FOREIGN KEY (environment_id) REFERENCES environment_plugin_installations,
    CONSTRAINT environment_plugin_installation_revisions_plugins_fk
        FOREIGN KEY (plugin_id) REFERENCES plugins
);

CREATE INDEX environment_plugin_installation_revisions_plugin_idx
    ON environment_plugin_installation_revisions (plugin_id);
