CREATE TABLE mcp_deployments
(
    mcp_deployment_id  TEXT      NOT NULL,
    environment_id     TEXT      NOT NULL,
    domain             TEXT      NOT NULL,

    created_at         TIMESTAMP NOT NULL,
    deleted_at         TIMESTAMP,
    modified_by        TEXT      NOT NULL,

    current_revision_id INTEGER   NOT NULL,

    PRIMARY KEY (mcp_deployment_id),
    FOREIGN KEY (environment_id) REFERENCES environments
);

CREATE UNIQUE INDEX mcp_deployments_domain_uk
    ON mcp_deployments (environment_id, domain)
    WHERE deleted_at IS NULL;

CREATE TABLE mcp_deployment_revisions
(
    mcp_deployment_id TEXT      NOT NULL,
    revision_id       INTEGER   NOT NULL,

    created_at        TIMESTAMP NOT NULL,
    created_by        TEXT      NOT NULL,
    deleted           BOOLEAN   NOT NULL,

    domain            TEXT      NOT NULL,

    PRIMARY KEY (mcp_deployment_id, revision_id),
    FOREIGN KEY (mcp_deployment_id) REFERENCES mcp_deployments
);

CREATE INDEX mcp_deployment_revisions_latest_revision_by_id_idx
    ON mcp_deployment_revisions (mcp_deployment_id, revision_id DESC);

CREATE TABLE deployment_compiled_mcp
(
    account_id                TEXT    NOT NULL,
    environment_id            TEXT    NOT NULL,
    deployment_revision_id    INTEGER NOT NULL,
    domain                    TEXT    NOT NULL,
    agent_type_implementers   TEXT    NOT NULL DEFAULT '{}',

    PRIMARY KEY (environment_id, deployment_revision_id, domain),
    FOREIGN KEY (environment_id, deployment_revision_id) REFERENCES deployment_revisions
);

CREATE INDEX deployment_compiled_mcp_domain_idx
    ON deployment_compiled_mcp (domain);
