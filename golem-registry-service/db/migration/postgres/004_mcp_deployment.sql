CREATE TABLE mcp_deployments
(
    mcp_deployment_id  UUID      NOT NULL,
    environment_id     UUID      NOT NULL,
    domain             TEXT      NOT NULL,

    created_at         TIMESTAMP NOT NULL,
    deleted_at         TIMESTAMP,
    modified_by        UUID      NOT NULL,

    current_revision_id BIGINT    NOT NULL,

    CONSTRAINT mcp_deployments_pk
        PRIMARY KEY (mcp_deployment_id),
    CONSTRAINT mcp_deployments_environments_fk
        FOREIGN KEY (environment_id) REFERENCES environments
);

CREATE UNIQUE INDEX mcp_deployments_domain_uk
    ON mcp_deployments (environment_id, domain)
    WHERE deleted_at IS NULL;

CREATE TABLE mcp_deployment_revisions
(
    mcp_deployment_id UUID      NOT NULL,
    revision_id       BIGINT    NOT NULL,

    created_at        TIMESTAMP NOT NULL,
    created_by        UUID      NOT NULL,
    deleted           BOOLEAN   NOT NULL,

    domain            TEXT      NOT NULL,

    CONSTRAINT mcp_deployment_revisions_pk
        PRIMARY KEY (mcp_deployment_id, revision_id),
    CONSTRAINT mcp_deployment_revisions_deployments_fk
        FOREIGN KEY (mcp_deployment_id) REFERENCES mcp_deployments
);

CREATE INDEX mcp_deployment_revisions_latest_revision_by_id_idx
    ON mcp_deployment_revisions (mcp_deployment_id, revision_id DESC);
