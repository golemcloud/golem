CREATE TABLE deployment_registered_tools
(
    environment_id         UUID   NOT NULL,
    deployment_revision_id BIGINT NOT NULL,
    tool_name              TEXT   NOT NULL,
    component_id           UUID   NOT NULL,
    component_revision_id  BIGINT NOT NULL,
    tool_definition        BLOB   NOT NULL,
    tool_provision_config  BLOB   NOT NULL,
    metadata_version       TEXT   NOT NULL,

    CONSTRAINT deployment_registered_tools_pk
        PRIMARY KEY (environment_id, deployment_revision_id, tool_name),
    CONSTRAINT deployment_registered_tools_component_revision_fk
        FOREIGN KEY (environment_id, deployment_revision_id, component_id, component_revision_id)
            REFERENCES deployment_component_revisions
                (environment_id, deployment_revision_id, component_id, component_revision_id)
);

CREATE INDEX deployment_registered_tools_component_idx
    ON deployment_registered_tools (component_id, component_revision_id, deployment_revision_id DESC);

CREATE TABLE deployment_agent_tool_bindings
(
    environment_id         UUID   NOT NULL,
    deployment_revision_id BIGINT NOT NULL,
    agent_type_name        TEXT   NOT NULL,
    tool_name              TEXT   NOT NULL,
    compiled_binding       BLOB   NOT NULL,

    CONSTRAINT deployment_agent_tool_bindings_pk
        PRIMARY KEY (environment_id, deployment_revision_id, agent_type_name, tool_name),
    CONSTRAINT deployment_agent_tool_bindings_agent_type_fk
        FOREIGN KEY (environment_id, deployment_revision_id, agent_type_name)
            REFERENCES deployment_registered_agent_types
                (environment_id, deployment_revision_id, agent_type_name),
    CONSTRAINT deployment_agent_tool_bindings_tool_fk
        FOREIGN KEY (environment_id, deployment_revision_id, tool_name)
            REFERENCES deployment_registered_tools
                (environment_id, deployment_revision_id, tool_name)
);

CREATE INDEX deployment_agent_tool_bindings_tool_idx
    ON deployment_agent_tool_bindings (environment_id, deployment_revision_id, tool_name, agent_type_name);
