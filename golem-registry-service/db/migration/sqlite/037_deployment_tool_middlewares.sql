CREATE TABLE deployment_tool_middleware_snapshots
(
    environment_id TEXT NOT NULL,
    deployment_revision_id INTEGER NOT NULL,
    registered_middlewares BLOB NOT NULL,
    compiled_chains BLOB NOT NULL,
    universal_installations BLOB NOT NULL,
    compatibility_mode BLOB NOT NULL,
    published_names BLOB NOT NULL,
    remote_names BLOB NOT NULL,
    PRIMARY KEY (environment_id, deployment_revision_id),
    FOREIGN KEY (environment_id, deployment_revision_id)
        REFERENCES deployment_revisions (environment_id, revision_id)
);
