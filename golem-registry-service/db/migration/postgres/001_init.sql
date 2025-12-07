CREATE TABLE plans
(
    plan_id                              UUID   NOT NULL,
    name                                 TEXT   NOT NULL,

    max_memory_per_worker                BIGINT NOT NULL,
    total_app_count                      BIGINT NOT NULL,
    total_env_count                      BIGINT NOT NULL,
    total_component_count                BIGINT NOT NULL,
    total_worker_count                   BIGINT NOT NULL,
    total_worker_connection_count        BIGINT NOT NULL,
    total_component_storage_bytes        BIGINT NOT NULL,
    monthly_gas_limit                    BIGINT NOT NULL,
    monthly_component_upload_limit_bytes BIGINT NOT NULL,

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
VALUES (0, 'TOTAL_WORKER_COUNT'),
       (1, 'TOTAL_WORKER_CONNECTION_COUNT'),
       (2, 'MONTHLY_GAS_LIMIT'),
       (3, 'MONTHLY_COMPONENT_UPLOAD_LIMIT_BYTES');

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
    -- Bitvector of roles
    roles       INT       NOT NULL,

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

CREATE TABLE tokens
(
    token_id   UUID      NOT NULL,
    secret     TEXT      NOT NULL,
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
    CONSTRAINT account_usage_stats_usage_types
        FOREIGN KEY (usage_type) REFERENCES usage_types
);

CREATE INDEX account_usage_stats_usage_type_idx ON account_usage_stats (usage_type);

CREATE TABLE applications
(
    application_id      UUID      NOT NULL,
    name                TEXT      NOT NULL,
    account_id          UUID      NOT NULL,

    created_at          TIMESTAMP NOT NULL,
    updated_at          TIMESTAMP NOT NULL,
    deleted_at          TIMESTAMP,
    modified_by         UUID      NOT NULL,

    current_revision_id BIGINT    NOT NULL,

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
    name                TEXT      NOT NULL,

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
    ON components (environment_id, name);

CREATE TABLE component_revisions
(
    component_id                 UUID      NOT NULL,
    revision_id                  BIGINT    NOT NULL,
    version                      TEXT      NOT NULL,

    hash                         BYTEA     NOT NULL,

    created_at                   TIMESTAMP NOT NULL,
    created_by                   UUID      NOT NULL,
    deleted                      BOOLEAN   NOT NULL,

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
    component_id      UUID      NOT NULL,
    revision_id       BIGINT    NOT NULL,
    file_path         TEXT      NOT NULL,

    created_at        TIMESTAMP NOT NULL,
    created_by        UUID      NOT NULL,

    file_content_hash BYTEA     NOT NULL,
    file_permissions  TEXT      NOT NULL,

    CONSTRAINT original_component_files_pk
        PRIMARY KEY (component_id, revision_id, file_path),
    CONSTRAINT original_component_files_components_fk
        FOREIGN KEY (component_id, revision_id) REFERENCES component_revisions
);

CREATE TABLE component_files
(
    component_id      UUID      NOT NULL,
    revision_id       BIGINT    NOT NULL,
    file_path         TEXT      NOT NULL,

    created_at        TIMESTAMP NOT NULL,
    created_by        UUID      NOT NULL,

    file_content_hash BYTEA     NOT NULL,
    file_permissions  TEXT      NOT NULL,

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

    deployment_revision_id BIGINT    NOT NULL,

    created_at             TIMESTAMP NOT NULL,
    created_by             UUID      NOT NULL,
    CONSTRAINT current_deployment_revisions_pk
        PRIMARY KEY (environment_id, revision_id),
    CONSTRAINT current_deployment_revisions_deployment_revisions_fk
        FOREIGN KEY (environment_id, deployment_revision_id) REFERENCES deployment_revisions (environment_id, revision_id)
);

CREATE INDEX current_deployment_revisions_environment_idx
    ON current_deployment_revisions (environment_id);

CREATE INDEX current_deployment_revisions_current_idx
    ON current_deployment_revisions (environment_id, deployment_revision_id);

CREATE INDEX current_deployment_revisions_latest_idx
    ON current_deployment_revisions (environment_id, revision_id DESC);

CREATE TABLE current_deployments
(
    environment_id      UUID   NOT NULL,
    current_revision_id BIGINT NOT NULL,
    CONSTRAINT current_deployments_pk
        PRIMARY KEY (environment_id),
    CONSTRAINT current_deployments_deployments_revisions_fk
        FOREIGN KEY (environment_id, current_revision_id) REFERENCES current_deployment_revisions (environment_id, revision_id)
);

CREATE INDEX current_deployments_environment_current_revision_idx
    ON current_deployments (environment_id, current_revision_id);

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
    ON http_api_definitions (environment_id, name);

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
    domain                 TEXT      NOT NULL,

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

CREATE UNIQUE INDEX http_api_deployments_domain_uk
    ON http_api_deployments (environment_id, domain);

CREATE TABLE http_api_deployment_revisions
(
    http_api_deployment_id UUID      NOT NULL,
    revision_id            BIGINT    NOT NULL,

    hash                   BYTEA     NOT NULL,

    created_at             TIMESTAMP NOT NULL,
    created_by             UUID      NOT NULL,
    deleted                BOOLEAN   NOT NULL,

    http_api_definitions   TEXT      NOT NULL,

    CONSTRAINT http_api_deployment_revisions_pk
        PRIMARY KEY (http_api_deployment_id, revision_id),
    CONSTRAINT http_api_deployment_revisions_deployments_fk
        FOREIGN KEY (http_api_deployment_id) REFERENCES http_api_deployments
);

CREATE INDEX http_api_deployment_revisions_latest_revision_by_id_idx
    ON http_api_deployment_revisions (http_api_deployment_id, revision_id DESC);

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
    deleted_at            TIMESTAMP,
    deleted_by            UUID,

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
    wasm_content_hash     BYTEA,

    CONSTRAINT plugins_pk
        PRIMARY KEY (plugin_id),
    CONSTRAINT plugins_components_fk
        FOREIGN KEY (component_id, component_revision_id)
            REFERENCES component_revisions (component_id, revision_id),
    CONSTRAINT plugins_accounts_fk
        FOREIGN KEY (account_id) REFERENCES accounts
);

CREATE UNIQUE INDEX plugins_name_version_uk ON plugins (account_id, name, version)
    WHERE deleted_at IS NULL;

CREATE INDEX plugins_component_idx ON plugins (component_id, component_revision_id);

CREATE TABLE environment_shares
(
    environment_share_id UUID      NOT NULL,
    environment_id       UUID      NOT NULL,
    grantee_account_id   UUID      NOT NULL,

    created_at           TIMESTAMP NOT NULL,
    updated_at           TIMESTAMP NOT NULL,
    deleted_at           TIMESTAMP,
    modified_by          UUID      NOT NULL,

    current_revision_id  BIGINT    NOT NULL,

    CONSTRAINT environment_shares_pk
        PRIMARY KEY (environment_share_id),
    CONSTRAINT environment_shares_environments_fk
        FOREIGN KEY (environment_id) REFERENCES environments,
    CONSTRAINT environment_shares_accounts_fk
        FOREIGN KEY (grantee_account_id) REFERENCES accounts (account_id)
);

CREATE UNIQUE INDEX environment_shares_environment_grantee_uk
    ON environment_shares (environment_id, grantee_account_id)
    WHERE deleted_at IS NULL;

CREATE INDEX environment_shares_environment_idx
    ON environment_shares (environment_id);

CREATE INDEX environment_shares_grantee_idx
    ON environment_shares (grantee_account_id);

CREATE TABLE environment_share_revisions
(
    environment_share_id UUID      NOT NULL,
    revision_id          BIGINT    NOT NULL,

    -- Bitvector of roles
    roles                INT       NOT NULL,

    created_at           TIMESTAMP NOT NULL,
    created_by           UUID      NOT NULL,
    deleted              BOOLEAN   NOT NULL,

    CONSTRAINT environment_share_revisions_pk
        PRIMARY KEY (environment_share_id, revision_id),
    CONSTRAINT environment_share_revisions_environment_shares_fk
        FOREIGN KEY (environment_share_id) REFERENCES environment_shares
);

CREATE TABLE environment_plugin_grants
(
    environment_plugin_grant_id UUID      NOT NULL,
    environment_id              UUID      NOT NULL,
    plugin_id                   UUID      NOT NULL,

    created_at                  TIMESTAMP NOT NULL,
    created_by                  UUID      NOT NULL,
    deleted_at                  TIMESTAMP,
    deleted_by                  UUID,

    CONSTRAINT environment_plugin_grants_pk
        PRIMARY KEY (environment_plugin_grant_id),
    CONSTRAINT environment_plugin_grants_environments_fk
        FOREIGN KEY (environment_id) REFERENCES environments,
    CONSTRAINT environment_plugin_grants_plugins_fk
        FOREIGN KEY (plugin_id) REFERENCES plugins
);

CREATE UNIQUE INDEX environment_plugin_grants_environment_plugin_uk
    ON environment_plugin_grants (environment_id, plugin_id)
    WHERE deleted_at IS NULL;

CREATE INDEX environment_plugin_grants_environment_id_idx ON environment_plugin_grants (environment_id);
CREATE INDEX environment_plugin_grants_plugin_id_idx ON environment_plugin_grants (plugin_id);

CREATE TABLE component_plugin_installations
(
    component_id UUID      NOT NULL,
    revision_id  BIGINT    NOT NULL,
    priority     INT       NOT NULL,

    created_at   TIMESTAMP NOT NULL,
    created_by   UUID      NOT NULL,

    environment_plugin_grant_id UUID NOT NULL,
    parameters   JSONB     NOT NULL,

    CONSTRAINT component_plugin_installations_pk
        PRIMARY KEY (component_id, revision_id, priority),
    CONSTRAINT component_plugin_installations_components_fk
        FOREIGN KEY (component_id, revision_id) REFERENCES component_revisions,
    CONSTRAINT component_plugin_installations_environment_plugin_grant_id_fk
        FOREIGN KEY (environment_plugin_grant_id) REFERENCES environment_plugin_grants
);

CREATE TABLE domain_registrations
(
    domain_registration_id UUID      NOT NULL,
    environment_id         UUID      NOT NULL,
    domain                 TEXT      NOT NULL,

    created_at             TIMESTAMP NOT NULL,
    created_by             UUID      NOT NULL,
    deleted_at             TIMESTAMP,
    deleted_by             UUID,

    CONSTRAINT domain_registrations_pk
        PRIMARY KEY (domain_registration_id),
    CONSTRAINT domain_registrations_environments_fk
        FOREIGN KEY (environment_id) REFERENCES environments
);

CREATE UNIQUE INDEX domain_registrations_domain_uk
    ON domain_registrations (domain)
    WHERE deleted_at IS NULL;

CREATE INDEX domain_registrations_environment_id_idx ON domain_registrations (environment_id);

CREATE INDEX domain_registrations_env_domain_active_idx
    ON domain_registrations (environment_id, domain)
    WHERE deleted_at IS NULL;

CREATE TABLE security_schemes
(
    security_scheme_id  UUID      NOT NULL,
    environment_id      UUID      NOT NULL,
    name                TEXT      NOT NULL,

    created_at          TIMESTAMP NOT NULL,
    updated_at          TIMESTAMP NOT NULL,
    deleted_at          TIMESTAMP,
    modified_by         UUID      NOT NULL,

    current_revision_id BIGINT    NOT NULL,

    CONSTRAINT security_schemes_pk
        PRIMARY KEY (security_scheme_id),
    CONSTRAINT security_schemes_environments_fk
        FOREIGN KEY (environment_id) REFERENCES environments
);

CREATE UNIQUE INDEX security_schemes_environment_name_uk
    ON security_schemes (environment_id, name)
    WHERE deleted_at IS NULL;

CREATE INDEX security_schemes_environment_idx
    ON security_schemes (environment_id);

CREATE TABLE security_scheme_revisions
(
    security_scheme_id UUID      NOT NULL,
    revision_id        BIGINT    NOT NULL,

    provider_type      TEXT      NOT NULL,
    client_id          TEXT      NOT NULL,
    client_secret      TEXT      NOT NULL,
    redirect_url       TEXT      NOT NULL,
    -- string containing a json array
    scopes             TEXT      NOT NULL,

    created_at         TIMESTAMP NOT NULL,
    created_by         UUID      NOT NULL,
    deleted            BOOLEAN   NOT NULL,

    CONSTRAINT security_scheme_revisions_pk
        PRIMARY KEY (security_scheme_id, revision_id),
    CONSTRAINT security_schemes_revisions_security_schemes_fk
        FOREIGN KEY (security_scheme_id) REFERENCES security_schemes
);

CREATE TABLE deployment_domain_http_api_definitions
(
    environment_id         UUID   NOT NULL,
    deployment_revision_id BIGINT NOT NULL,
    domain                 TEXT   NOT NULL,
    http_api_definition_id UUID   NOT NULL,

    CONSTRAINT deployment_domains_pk
        PRIMARY KEY (environment_id, deployment_revision_id, domain, http_api_definition_id),

    CONSTRAINT deployment_domains_environments_fk
        FOREIGN KEY (environment_id)
            REFERENCES environments (environment_id),

    CONSTRAINT deployment_domains_http_api_definitions_fk
        FOREIGN KEY (http_api_definition_id)
            REFERENCES http_api_definitions (http_api_definition_id),

    CONSTRAINT deployment_domains_deployment_revisions_fk
        FOREIGN KEY (environment_id, deployment_revision_id)
            REFERENCES deployment_revisions (environment_id, revision_id)
);

CREATE INDEX deployment_domain_http_api_definitions_domain_idx
    ON deployment_domain_http_api_definitions (domain);

CREATE TABLE deployment_compiled_http_api_definition_routes
(
    environment_id         UUID    NOT NULL,
    deployment_revision_id BIGINT  NOT NULL,
    http_api_definition_id UUID    NOT NULL,
    id                     INTEGER NOT NULL, -- enumerated per definition

    security_scheme        TEXT,             -- nullable if no security
    compiled_route         BYTEA   NOT NULL, -- full compiled route as blob

    CONSTRAINT deployment_routes_pk
        PRIMARY KEY (environment_id, deployment_revision_id, http_api_definition_id, id),

    CONSTRAINT deployment_routes_environments_fk
        FOREIGN KEY (environment_id)
            REFERENCES environments (environment_id),

    CONSTRAINT deployment_routes_http_api_definitions_fk
        FOREIGN KEY (http_api_definition_id)
            REFERENCES http_api_definitions (http_api_definition_id),

    CONSTRAINT deployment_routes_deployment_revisions_fk
        FOREIGN KEY (environment_id, deployment_revision_id)
            REFERENCES deployment_revisions (environment_id, revision_id)
);

CREATE INDEX deployment_routes_routes_def_idx
    ON deployment_compiled_http_api_definition_routes (environment_id, deployment_revision_id, http_api_definition_id);

CREATE TABLE deployment_registered_agent_types
(
    environment_id         UUID   NOT NULL,
    deployment_revision_id BIGINT NOT NULL,
    agent_type_name        TEXT   NOT NULL,

    component_id           UUID   NOT NULL, -- compoenent implementing agent type in this deployment
    agent_type             BYTEA  NOT NULL, -- full agent type as blob

    CONSTRAINT deployment_registered_agent_types_pk
        PRIMARY KEY (environment_id, deployment_revision_id, agent_type_name),

    CONSTRAINT deployment_registered_agent_types_environments_fk
        FOREIGN KEY (environment_id)
            REFERENCES environments (environment_id),

    CONSTRAINT deployment_registered_agent_types_components_fk
        FOREIGN KEY (component_id)
            REFERENCES components (component_id),

    CONSTRAINT deployment_registered_agent_types_deployment_revisions_fk
        FOREIGN KEY (environment_id, deployment_revision_id)
            REFERENCES deployment_revisions (environment_id, revision_id)
);

CREATE INDEX deployment_registered_agent_types_agent_type_idx
    ON deployment_registered_agent_types (environment_id, deployment_revision_id, agent_type_name);
