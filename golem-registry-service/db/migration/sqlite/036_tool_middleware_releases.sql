CREATE TABLE tool_middleware_releases
(
    tool_middleware_release_id UUID NOT NULL,
    owner_account_id UUID NOT NULL,
    tool_middleware_name TEXT NOT NULL,
    tool_version TEXT NOT NULL,
    source_kind SMALLINT NOT NULL,
    component_id UUID NOT NULL,
    component_revision BIGINT NOT NULL,
    component_name TEXT NOT NULL,
    host_tool_id TEXT,
    implementation_version TEXT,
    tool_definition BYTEA NOT NULL,
    metadata_version TEXT NOT NULL,
    metadata_digest BYTEA NOT NULL,
    immutable BOOLEAN NOT NULL,
    lifecycle SMALLINT NOT NULL,
    origin SMALLINT NOT NULL,
    system_availability SMALLINT,
    created_at TIMESTAMP NOT NULL,
    created_by UUID NOT NULL,
    state_changed_at TIMESTAMP NOT NULL,
    state_changed_by UUID NOT NULL,
    CONSTRAINT tool_middleware_releases_pk PRIMARY KEY (tool_middleware_release_id),
    CONSTRAINT tool_middleware_releases_owner_account_fk FOREIGN KEY (owner_account_id) REFERENCES accounts,
    CONSTRAINT tool_middleware_releases_component_revision_fk FOREIGN KEY (component_id, component_revision) REFERENCES component_revisions,
    CONSTRAINT tool_middleware_releases_source_kind_check CHECK (source_kind = 0),
    CONSTRAINT tool_middleware_releases_lifecycle_check CHECK (lifecycle IN (0, 1, 2)),
    CONSTRAINT tool_middleware_releases_origin_check CHECK (origin IN (0, 1)),
    CONSTRAINT tool_middleware_releases_no_system_availability_check CHECK (system_availability IS NULL),
    CONSTRAINT tool_middleware_releases_no_host_source_check CHECK (host_tool_id IS NULL AND implementation_version IS NULL)
);

CREATE UNIQUE INDEX tool_middleware_releases_owner_name_version_uk
    ON tool_middleware_releases (owner_account_id, tool_middleware_name, tool_version)
    WHERE lifecycle != 2;
CREATE INDEX tool_middleware_releases_component_revision_idx
    ON tool_middleware_releases (component_id, component_revision);

CREATE TRIGGER tool_middleware_releases_component_owner_insert_check
    BEFORE INSERT ON tool_middleware_releases
    WHEN NOT EXISTS (
        SELECT 1 FROM components c
        JOIN environments e ON e.environment_id = c.environment_id
        JOIN applications app ON app.application_id = e.application_id
        WHERE c.component_id = NEW.component_id
          AND app.account_id = NEW.owner_account_id
    )
BEGIN
    SELECT RAISE(ABORT, 'component tool middleware release source must belong to the release owner account');
END;

CREATE TRIGGER tool_middleware_releases_component_owner_update_check
    BEFORE UPDATE OF owner_account_id, component_id ON tool_middleware_releases
    WHEN NOT EXISTS (
        SELECT 1 FROM components c
        JOIN environments e ON e.environment_id = c.environment_id
        JOIN applications app ON app.application_id = e.application_id
        WHERE c.component_id = NEW.component_id
          AND app.account_id = NEW.owner_account_id
    )
BEGIN
    SELECT RAISE(ABORT, 'component tool middleware release source must belong to the release owner account');
END;

CREATE TABLE environment_tool_middleware_grants
(
    environment_tool_middleware_grant_id UUID NOT NULL,
    environment_id UUID NOT NULL,
    tool_middleware_release_id UUID NOT NULL,
    protected BOOLEAN NOT NULL,
    automatic BOOLEAN NOT NULL,
    follow_coordinates BOOLEAN NOT NULL,
    created_at TIMESTAMP NOT NULL,
    created_by UUID NOT NULL,
    state_changed_at TIMESTAMP NOT NULL,
    state_changed_by UUID NOT NULL,
    deleted_at TIMESTAMP,
    deleted_by UUID,
    CONSTRAINT environment_tool_middleware_grants_pk PRIMARY KEY (environment_tool_middleware_grant_id),
    CONSTRAINT environment_tool_middleware_grants_environment_fk FOREIGN KEY (environment_id) REFERENCES environments,
    CONSTRAINT environment_tool_middleware_grants_release_fk FOREIGN KEY (tool_middleware_release_id) REFERENCES tool_middleware_releases,
    CONSTRAINT environment_tool_middleware_grants_deletion_state_check CHECK ((deleted_at IS NULL AND deleted_by IS NULL) OR (deleted_at IS NOT NULL AND deleted_by IS NOT NULL))
);

CREATE UNIQUE INDEX environment_tool_middleware_grants_environment_release_uk
    ON environment_tool_middleware_grants (environment_id, tool_middleware_release_id);
CREATE INDEX environment_tool_middleware_grants_active_environment_idx
    ON environment_tool_middleware_grants (environment_id, deleted_at);
CREATE INDEX environment_tool_middleware_grants_active_release_idx
    ON environment_tool_middleware_grants (tool_middleware_release_id, deleted_at);
