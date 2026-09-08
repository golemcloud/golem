CREATE TABLE deployment_tool_middleware_snapshots
(
    environment_id UUID NOT NULL,
    deployment_revision_id BIGINT NOT NULL,
    registered_middlewares BYTEA NOT NULL,
    compiled_chains BYTEA NOT NULL,
    universal_installations BYTEA NOT NULL,
    compatibility_mode BYTEA NOT NULL,
    published_names BYTEA NOT NULL,
    remote_names BYTEA NOT NULL,
    PRIMARY KEY (environment_id, deployment_revision_id),
    FOREIGN KEY (environment_id, deployment_revision_id)
        REFERENCES deployment_revisions (environment_id, revision_id)
);
