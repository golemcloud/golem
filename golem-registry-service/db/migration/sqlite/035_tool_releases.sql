CREATE TABLE tool_releases
(
    tool_release_id       UUID      NOT NULL,
    owner_account_id      UUID      NOT NULL,
    tool_name             TEXT      NOT NULL,
    tool_version          TEXT      NOT NULL,
    source_kind           SMALLINT  NOT NULL,
    component_id          UUID,
    component_revision    BIGINT,
    component_name        TEXT,
    host_tool_id          TEXT,
    implementation_version TEXT,
    tool_definition       BLOB      NOT NULL,
    metadata_version      TEXT      NOT NULL,
    metadata_digest       BLOB      NOT NULL,
    lifecycle             SMALLINT  NOT NULL,
    origin                SMALLINT  NOT NULL,
    system_availability   SMALLINT,
    created_at            TIMESTAMP NOT NULL,
    created_by            UUID      NOT NULL,
    state_changed_at      TIMESTAMP NOT NULL,
    state_changed_by      UUID      NOT NULL,

    CONSTRAINT tool_releases_pk
        PRIMARY KEY (tool_release_id),
    CONSTRAINT tool_releases_owner_account_fk
        FOREIGN KEY (owner_account_id) REFERENCES accounts,
    CONSTRAINT tool_releases_component_revision_fk
        FOREIGN KEY (component_id, component_revision) REFERENCES component_revisions,
    CONSTRAINT tool_releases_source_kind_check
        CHECK (source_kind IN (0, 1)),
    CONSTRAINT tool_releases_source_fields_check
        CHECK (
            (source_kind = 0
                AND component_id IS NOT NULL
                AND component_revision IS NOT NULL
                AND component_name IS NOT NULL
                AND host_tool_id IS NULL
                AND implementation_version IS NULL)
            OR (source_kind = 1
                AND component_id IS NULL
                AND component_revision IS NULL
                AND component_name IS NULL
                AND host_tool_id IS NOT NULL
                AND implementation_version IS NOT NULL)
        ),
    CONSTRAINT tool_releases_lifecycle_check
        CHECK (lifecycle IN (0, 1)),
    CONSTRAINT tool_releases_origin_check
        CHECK (origin IN (0, 1)),
    CONSTRAINT tool_releases_system_availability_check
        CHECK (
            (origin = 0 AND system_availability IS NULL)
            OR (origin = 1
                AND system_availability IS NOT NULL
                AND system_availability IN (0, 1, 2))
        )
);

CREATE UNIQUE INDEX tool_releases_owner_name_version_uk
    ON tool_releases (owner_account_id, tool_name, tool_version);

CREATE INDEX tool_releases_component_revision_idx
    ON tool_releases (component_id, component_revision);

CREATE TRIGGER tool_releases_component_owner_check_insert
BEFORE INSERT ON tool_releases
WHEN NEW.source_kind = 0 AND NOT EXISTS (
    SELECT 1
    FROM components c
    JOIN environments e ON e.environment_id = c.environment_id
    JOIN applications app ON app.application_id = e.application_id
    WHERE c.component_id = NEW.component_id
      AND app.account_id = NEW.owner_account_id
)
BEGIN
    SELECT RAISE(ABORT, 'component tool release source must belong to the release owner account');
END;

CREATE TRIGGER tool_releases_component_owner_check_update
BEFORE UPDATE OF owner_account_id, source_kind, component_id ON tool_releases
WHEN NEW.source_kind = 0 AND NOT EXISTS (
    SELECT 1
    FROM components c
    JOIN environments e ON e.environment_id = c.environment_id
    JOIN applications app ON app.application_id = e.application_id
    WHERE c.component_id = NEW.component_id
      AND app.account_id = NEW.owner_account_id
)
BEGIN
    SELECT RAISE(ABORT, 'component tool release source must belong to the release owner account');
END;

CREATE TABLE environment_tool_grants
(
    environment_tool_grant_id UUID      NOT NULL,
    environment_id            UUID      NOT NULL,
    tool_release_id            UUID      NOT NULL,
    protected                  BOOLEAN   NOT NULL,
    automatic                  BOOLEAN   NOT NULL,
    lifecycle                  SMALLINT  NOT NULL,
    created_at                 TIMESTAMP NOT NULL,
    created_by                 UUID      NOT NULL,
    state_changed_at           TIMESTAMP NOT NULL,
    state_changed_by           UUID      NOT NULL,
    deleted_at                 TIMESTAMP,
    deleted_by                 UUID,

    CONSTRAINT environment_tool_grants_pk
        PRIMARY KEY (environment_tool_grant_id),
    CONSTRAINT environment_tool_grants_environment_fk
        FOREIGN KEY (environment_id) REFERENCES environments,
    CONSTRAINT environment_tool_grants_release_fk
        FOREIGN KEY (tool_release_id) REFERENCES tool_releases,
    CONSTRAINT environment_tool_grants_lifecycle_check
        CHECK (lifecycle IN (0, 1)),
    CONSTRAINT environment_tool_grants_deletion_state_check
        CHECK (
            (lifecycle = 0 AND deleted_at IS NULL AND deleted_by IS NULL)
            OR (lifecycle = 1 AND deleted_at IS NOT NULL AND deleted_by IS NOT NULL)
        )
);

CREATE UNIQUE INDEX environment_tool_grants_environment_release_uk
    ON environment_tool_grants (environment_id, tool_release_id);
CREATE INDEX environment_tool_grants_active_environment_idx
    ON environment_tool_grants (environment_id, deleted_at);
CREATE INDEX environment_tool_grants_active_release_idx
    ON environment_tool_grants (tool_release_id, deleted_at);

CREATE TEMP TABLE migration_034_tool_release_validation
(
    valid INTEGER NOT NULL CHECK (valid = 1)
);

INSERT INTO migration_034_tool_release_validation (valid)
SELECT 0
FROM deployment_registered_tools registered
LEFT JOIN component_revisions
  ON component_revisions.component_id = registered.component_id
 AND component_revisions.revision_id = registered.component_revision_id
LEFT JOIN components ON components.component_id = component_revisions.component_id
LEFT JOIN environments ON environments.environment_id = registered.environment_id
LEFT JOIN applications ON applications.application_id = environments.application_id
LEFT JOIN accounts ON accounts.account_id = applications.account_id
WHERE component_revisions.component_id IS NULL
   OR components.component_id IS NULL
   OR environments.environment_id IS NULL
   OR applications.application_id IS NULL
   OR accounts.account_id IS NULL
LIMIT 1;

DROP TABLE migration_034_tool_release_validation;

CREATE TABLE deployment_registered_tools_v2
(
    environment_id         UUID     NOT NULL,
    deployment_revision_id BIGINT   NOT NULL,
    tool_name              TEXT     NOT NULL,
    tool_release_id        UUID,
    source_kind            SMALLINT NOT NULL,
    component_id           UUID,
    component_revision_id  BIGINT,
    component_name         TEXT,
    host_tool_id           TEXT,
    implementation_version TEXT,
    owner_account_id       UUID     NOT NULL,
    owner_account_email    TEXT     NOT NULL,
    tool_definition        BLOB     NOT NULL,
    tool_provision_config  BLOB     NOT NULL,
    metadata_version       TEXT     NOT NULL,
    metadata_digest        BLOB,

    CONSTRAINT deployment_registered_tools_v2_pk
        PRIMARY KEY (environment_id, deployment_revision_id, tool_name),
    CONSTRAINT deployment_registered_tools_v2_deployment_fk
        FOREIGN KEY (environment_id, deployment_revision_id)
            REFERENCES deployment_revisions (environment_id, revision_id),
    CONSTRAINT deployment_registered_tools_v2_component_revision_fk
        FOREIGN KEY (component_id, component_revision_id)
            REFERENCES component_revisions (component_id, revision_id),
    CONSTRAINT deployment_registered_tools_v2_release_fk
        FOREIGN KEY (tool_release_id) REFERENCES tool_releases,
    CONSTRAINT deployment_registered_tools_v2_owner_fk
        FOREIGN KEY (owner_account_id) REFERENCES accounts,
    CONSTRAINT deployment_registered_tools_v2_source_kind_check
        CHECK (source_kind IN (0, 1)),
    CONSTRAINT deployment_registered_tools_v2_source_fields_check
        CHECK (
            (source_kind = 0
                AND component_id IS NOT NULL
                AND component_revision_id IS NOT NULL
                AND component_name IS NOT NULL
                AND host_tool_id IS NULL
                AND implementation_version IS NULL)
            OR (source_kind = 1
                AND component_id IS NULL
                AND component_revision_id IS NULL
                AND component_name IS NULL
                AND host_tool_id IS NOT NULL
                AND implementation_version IS NOT NULL)
        )
);

INSERT INTO deployment_registered_tools_v2 (
    environment_id, deployment_revision_id, tool_name,
    tool_release_id, source_kind,
    component_id, component_revision_id, component_name,
    host_tool_id, implementation_version,
    owner_account_id, owner_account_email,
    tool_definition, tool_provision_config, metadata_version, metadata_digest
)
SELECT registered.environment_id,
       registered.deployment_revision_id,
       registered.tool_name,
       NULL,
       0,
       registered.component_id,
       registered.component_revision_id,
       components.name,
       NULL,
       NULL,
       applications.account_id,
       accounts.email,
       registered.tool_definition,
       registered.tool_provision_config,
       registered.metadata_version,
       NULL
FROM deployment_registered_tools registered
JOIN component_revisions
  ON component_revisions.component_id = registered.component_id
 AND component_revisions.revision_id = registered.component_revision_id
JOIN components ON components.component_id = component_revisions.component_id
JOIN environments ON environments.environment_id = registered.environment_id
JOIN applications ON applications.application_id = environments.application_id
JOIN accounts ON accounts.account_id = applications.account_id;

CREATE TABLE deployment_agent_tool_bindings_v2
(
    environment_id         UUID   NOT NULL,
    deployment_revision_id BIGINT NOT NULL,
    agent_type_name        TEXT   NOT NULL,
    tool_name              TEXT   NOT NULL,
    compiled_binding       BLOB   NOT NULL,

    CONSTRAINT deployment_agent_tool_bindings_v2_pk
        PRIMARY KEY (environment_id, deployment_revision_id, agent_type_name, tool_name),
    CONSTRAINT deployment_agent_tool_bindings_v2_agent_type_fk
        FOREIGN KEY (environment_id, deployment_revision_id, agent_type_name)
            REFERENCES deployment_registered_agent_types
                (environment_id, deployment_revision_id, agent_type_name),
    CONSTRAINT deployment_agent_tool_bindings_v2_tool_fk
        FOREIGN KEY (environment_id, deployment_revision_id, tool_name)
            REFERENCES deployment_registered_tools_v2
                (environment_id, deployment_revision_id, tool_name)
);

INSERT INTO deployment_agent_tool_bindings_v2
SELECT environment_id, deployment_revision_id, agent_type_name, tool_name, compiled_binding
FROM deployment_agent_tool_bindings;

DROP TABLE deployment_agent_tool_bindings;
DROP TABLE deployment_registered_tools;
ALTER TABLE deployment_registered_tools_v2 RENAME TO deployment_registered_tools;
ALTER TABLE deployment_agent_tool_bindings_v2 RENAME TO deployment_agent_tool_bindings;

CREATE INDEX deployment_registered_tools_component_idx
    ON deployment_registered_tools (component_id, component_revision_id, deployment_revision_id DESC);
CREATE INDEX deployment_registered_tools_release_idx
    ON deployment_registered_tools (tool_release_id, deployment_revision_id DESC);
CREATE INDEX deployment_agent_tool_bindings_tool_idx
    ON deployment_agent_tool_bindings (environment_id, deployment_revision_id, tool_name, agent_type_name);
