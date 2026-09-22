DROP TABLE deployment_agent_tool_bindings;

CREATE TABLE deployment_tool_bindings
(
    environment_id         UUID   NOT NULL,
    deployment_revision_id BIGINT NOT NULL,
    binding_owner          TEXT   NOT NULL,
    tool_name              TEXT   NOT NULL,
    compiled_binding       BYTEA  NOT NULL,

    CONSTRAINT deployment_tool_bindings_pk
        PRIMARY KEY (environment_id, deployment_revision_id, binding_owner, tool_name),
    CONSTRAINT deployment_tool_bindings_tool_fk
        FOREIGN KEY (environment_id, deployment_revision_id, tool_name)
            REFERENCES deployment_registered_tools
                (environment_id, deployment_revision_id, tool_name)
);

CREATE INDEX deployment_tool_bindings_tool_idx
    ON deployment_tool_bindings (environment_id, deployment_revision_id, tool_name, binding_owner);
