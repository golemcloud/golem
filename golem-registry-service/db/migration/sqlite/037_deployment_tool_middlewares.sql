CREATE TABLE deployment_tool_middleware_snapshots
(
    environment_id TEXT NOT NULL,
    deployment_revision_id INTEGER NOT NULL,
    registered_middlewares BLOB NOT NULL,
    compiled_chains BLOB NOT NULL,
    compatibility_mode TEXT NOT NULL,
    PRIMARY KEY (environment_id, deployment_revision_id),
    FOREIGN KEY (environment_id, deployment_revision_id)
        REFERENCES deployment_revisions (environment_id, revision_id)
);

CREATE TABLE deployment_tool_middleware_names
(
    environment_id TEXT NOT NULL,
    deployment_revision_id INTEGER NOT NULL,
    middleware_name TEXT NOT NULL,
    kind TEXT NOT NULL,
    PRIMARY KEY (environment_id, deployment_revision_id, middleware_name, kind),
    FOREIGN KEY (environment_id, deployment_revision_id)
        REFERENCES deployment_tool_middleware_snapshots (environment_id, deployment_revision_id)
);

CREATE TABLE deployment_tool_middleware_bindings
(
    environment_id TEXT NOT NULL,
    deployment_revision_id INTEGER NOT NULL,
    scope TEXT NOT NULL,
    agent_type_name TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    merge_mode TEXT,
    has_installations BOOLEAN NOT NULL,
    PRIMARY KEY (environment_id, deployment_revision_id, scope, agent_type_name, tool_name),
    FOREIGN KEY (environment_id, deployment_revision_id)
        REFERENCES deployment_tool_middleware_snapshots (environment_id, deployment_revision_id)
);

CREATE TABLE deployment_tool_middleware_installations
(
    environment_id TEXT NOT NULL,
    deployment_revision_id INTEGER NOT NULL,
    scope TEXT NOT NULL,
    agent_type_name TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    installation_index INTEGER NOT NULL,
    middleware_name TEXT NOT NULL,
    middleware_version TEXT,
    parameters BLOB NOT NULL,
    account_email TEXT,
    filesystem_access TEXT NOT NULL,
    PRIMARY KEY (environment_id, deployment_revision_id, scope, agent_type_name, tool_name, installation_index),
    FOREIGN KEY (environment_id, deployment_revision_id, scope, agent_type_name, tool_name)
        REFERENCES deployment_tool_middleware_bindings (environment_id, deployment_revision_id, scope, agent_type_name, tool_name)
);
