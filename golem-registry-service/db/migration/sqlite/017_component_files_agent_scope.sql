DROP TABLE component_files;

CREATE TABLE component_files
(
    component_id      UUID      NOT NULL,
    revision_id       BIGINT    NOT NULL,
    agent_type_name   TEXT      NOT NULL,
    file_path         TEXT      NOT NULL,

    created_at        TIMESTAMP NOT NULL,
    created_by        UUID      NOT NULL,

    file_content_hash BYTEA     NOT NULL,
    file_permissions  TEXT      NOT NULL,
    file_size         INTEGER   NOT NULL,

    CONSTRAINT component_files_pk
        PRIMARY KEY (component_id, revision_id, agent_type_name, file_path),
    CONSTRAINT component_files_components_fk
        FOREIGN KEY (component_id, revision_id) REFERENCES component_revisions
);
