CREATE TABLE resource_definitions
(
    resource_definition_id UUID NOT NULL,
    environment_id UUID NOT NULL,
    limit_type TEXT NOT NULL,
    name TEXT NOT NULL,

    created_at TIMESTAMP NOT NULL,
    updated_at TIMESTAMP NOT NULL,
    deleted_at TIMESTAMP,
    modified_by UUID NOT NULL,

    current_revision_id BIGINT NOT NULL,

    CONSTRAINT resource_definitions_pk
        PRIMARY KEY (resource_definition_id),
    CONSTRAINT resource_definitions_fk
        FOREIGN KEY (environment_id) REFERENCES environments
);

CREATE UNIQUE INDEX resource_definitions_name_uk
    ON resource_definitions (environment_id, name) WHERE deleted_at IS NULL;

CREATE TABLE resource_definition_revisions
(
    resource_definition_id UUID NOT NULL,
    revision_id BIGINT NOT NULL,

    hash BYTEA NOT NULL,

    created_at TIMESTAMP NOT NULL,
    created_by UUID NOT NULL,
    deleted BOOLEAN NOT NULL,

    limit_value NUMERIC NOT NULL,
    limit_period TEXT,

    enforcement_action TEXT NOT NULL,

    unit TEXT NOT NULL,
    units TEXT NOT NULL,

    CONSTRAINT resource_definition_revisions_pk
        PRIMARY KEY (resource_definition_id, revision_id),
    CONSTRAINT resource_definition_revisions_resource_definitions_fk
        FOREIGN KEY (resource_definition_id) REFERENCES resource_definitions
);

ALTER TABLE registry_change_events
    ADD resource_definition_id UUID;

ALTER TABLE registry_change_events
    ADD resource_name TEXT;
