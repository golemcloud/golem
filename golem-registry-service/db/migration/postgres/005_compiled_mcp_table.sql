-- Create table for storing compiled MCP configurations
-- agent_type_implementers is a JSON string mapping agent type names to their component implementations
CREATE TABLE deployment_compiled_mcp
(
    account_id                UUID   NOT NULL,
    environment_id            UUID   NOT NULL,
    deployment_revision_id    BIGINT NOT NULL,
    domain                    TEXT   NOT NULL,
    agent_type_implementers   TEXT   NOT NULL DEFAULT '{}',

    CONSTRAINT deployment_compiled_mcp_pk
        PRIMARY KEY (environment_id, deployment_revision_id, domain),
    CONSTRAINT deployment_compiled_mcp_deployments_fk
        FOREIGN KEY (environment_id, deployment_revision_id) REFERENCES deployment_revisions
);

CREATE INDEX deployment_compiled_mcp_domain_idx
    ON deployment_compiled_mcp (domain);
