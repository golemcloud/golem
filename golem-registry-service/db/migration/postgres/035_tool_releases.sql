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
    tool_definition       BYTEA     NOT NULL,
    metadata_version      TEXT      NOT NULL,
    metadata_digest       BYTEA     NOT NULL,
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

CREATE FUNCTION validate_tool_release_component_owner() RETURNS TRIGGER AS $$
BEGIN
    IF NEW.source_kind = 0 AND NOT EXISTS (
        SELECT 1
        FROM components c
        JOIN environments e ON e.environment_id = c.environment_id
        JOIN applications app ON app.application_id = e.application_id
        WHERE c.component_id = NEW.component_id
          AND app.account_id = NEW.owner_account_id
    ) THEN
        RAISE EXCEPTION 'component tool release source must belong to the release owner account';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tool_releases_component_owner_check
    BEFORE INSERT OR UPDATE OF owner_account_id, source_kind, component_id
    ON tool_releases
    FOR EACH ROW EXECUTE FUNCTION validate_tool_release_component_owner();

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

ALTER TABLE deployment_registered_tools
    DROP CONSTRAINT deployment_registered_tools_component_revision_fk;

ALTER TABLE deployment_registered_tools
    ADD COLUMN tool_release_id UUID,
    ADD COLUMN source_kind SMALLINT NOT NULL DEFAULT 0,
    ADD COLUMN component_name TEXT,
    ADD COLUMN host_tool_id TEXT,
    ADD COLUMN implementation_version TEXT,
    ADD COLUMN owner_account_id UUID,
    ADD COLUMN owner_account_email TEXT,
    ADD COLUMN metadata_digest BYTEA;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
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
    ) THEN
        RAISE EXCEPTION 'deployment_registered_tools contains rows with unresolved component ownership';
    END IF;
END;
$$;

UPDATE deployment_registered_tools registered
SET component_name = components.name,
    owner_account_id = applications.account_id,
    owner_account_email = accounts.email
FROM component_revisions, components, environments, applications, accounts
WHERE components.component_id = component_revisions.component_id
  AND environments.environment_id = registered.environment_id
  AND applications.application_id = environments.application_id
  AND accounts.account_id = applications.account_id
  AND component_revisions.component_id = registered.component_id
  AND component_revisions.revision_id = registered.component_revision_id;

ALTER TABLE deployment_registered_tools
    ALTER COLUMN source_kind DROP DEFAULT,
    ALTER COLUMN component_id DROP NOT NULL,
    ALTER COLUMN component_revision_id DROP NOT NULL,
    ALTER COLUMN owner_account_id SET NOT NULL,
    ALTER COLUMN owner_account_email SET NOT NULL,
    ADD CONSTRAINT deployment_registered_tools_source_kind_check
        CHECK (source_kind IN (0, 1)),
    ADD CONSTRAINT deployment_registered_tools_source_fields_check
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
        ),
    ADD CONSTRAINT deployment_registered_tools_component_revision_fk
        FOREIGN KEY (component_id, component_revision_id)
            REFERENCES component_revisions (component_id, revision_id),
    ADD CONSTRAINT deployment_registered_tools_deployment_fk
        FOREIGN KEY (environment_id, deployment_revision_id)
            REFERENCES deployment_revisions (environment_id, revision_id),
    ADD CONSTRAINT deployment_registered_tools_release_fk
        FOREIGN KEY (tool_release_id) REFERENCES tool_releases,
    ADD CONSTRAINT deployment_registered_tools_owner_fk
        FOREIGN KEY (owner_account_id) REFERENCES accounts;
