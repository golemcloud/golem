CREATE TABLE deployment_mcp_imports (
    environment_id UUID NOT NULL,
    deployment_revision_id BIGINT NOT NULL,
    import_index BIGINT NOT NULL,
    import_hash BYTEA NOT NULL,
    import_config BYTEA NOT NULL,
    inline_credential BYTEA,
    CONSTRAINT deployment_mcp_imports_pk PRIMARY KEY (environment_id, deployment_revision_id, import_index),
    CONSTRAINT deployment_mcp_imports_deployment_revisions_fk FOREIGN KEY (environment_id, deployment_revision_id)
        REFERENCES deployment_revisions (environment_id, revision_id)
);
