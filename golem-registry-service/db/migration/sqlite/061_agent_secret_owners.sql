CREATE TABLE agent_secrets_v2
(
    agent_secret_id UUID NOT NULL,
    environment_id UUID NOT NULL,
    path JSONB NOT NULL,
    agent_secret_data BYTEA NOT NULL,
    created_at TIMESTAMP NOT NULL,
    updated_at TIMESTAMP NOT NULL,
    deleted_at TIMESTAMP,
    modified_by UUID NOT NULL,
    current_revision_id BIGINT NOT NULL,
    owner_kind TEXT NOT NULL,
    agent_id UUID,
    agent_fingerprint UUID,
    agent_type BIGINT,
    tool_name TEXT,

    CONSTRAINT agent_secrets_pk PRIMARY KEY (agent_secret_id),
    CONSTRAINT agent_secrets_environments_fk FOREIGN KEY (environment_id) REFERENCES environments,
    CONSTRAINT agent_secrets_owner_kind_check CHECK (owner_kind IN ('environment', 'agent', 'tool')),
    CONSTRAINT agent_secrets_owner_fields_check
        CHECK (
            (owner_kind = 'environment' AND agent_id IS NULL AND agent_fingerprint IS NULL
                AND agent_type IS NULL AND tool_name IS NULL)
            OR (owner_kind = 'agent' AND agent_id IS NOT NULL AND agent_fingerprint IS NOT NULL
                AND agent_type IS NOT NULL AND tool_name IS NULL)
            OR (owner_kind = 'tool' AND agent_id IS NULL AND agent_fingerprint IS NULL
                AND agent_type IS NULL AND tool_name IS NOT NULL)
        ),
    CONSTRAINT agent_secrets_agent_type_check CHECK (agent_type BETWEEN 1 AND 4294967295)
);

CREATE TABLE agent_secret_revisions_v2
(
    agent_secret_id UUID NOT NULL,
    revision_id BIGINT NOT NULL,
    agent_secret_revision_data BYTEA NOT NULL,
    created_at TIMESTAMP NOT NULL,
    created_by UUID NOT NULL,
    deleted BOOLEAN NOT NULL,
    destinations JSONB NOT NULL,

    CONSTRAINT agent_secret_revisions_pk PRIMARY KEY (agent_secret_id, revision_id),
    CONSTRAINT agent_secret_revisions_agent_secrets_fk
        FOREIGN KEY (agent_secret_id) REFERENCES agent_secrets_v2
);

INSERT INTO agent_secrets_v2
    (agent_secret_id, environment_id, path, agent_secret_data,
     created_at, updated_at, deleted_at, modified_by, current_revision_id, owner_kind)
SELECT agent_secret_id, environment_id, path, agent_secret_data,
       created_at, updated_at, deleted_at, modified_by, current_revision_id, 'environment'
FROM agent_secrets;

INSERT INTO agent_secret_revisions_v2
    (agent_secret_id, revision_id, agent_secret_revision_data, created_at, created_by, deleted, destinations)
SELECT agent_secret_id, revision_id, agent_secret_revision_data, created_at, created_by, deleted, '[]'
FROM agent_secret_revisions;

DROP TABLE agent_secret_revisions;
DROP TABLE agent_secrets;
ALTER TABLE agent_secrets_v2 RENAME TO agent_secrets;
ALTER TABLE agent_secret_revisions_v2 RENAME TO agent_secret_revisions;

CREATE UNIQUE INDEX agent_secrets_environment_path_uk ON agent_secrets (environment_id, path)
    WHERE deleted_at IS NULL AND owner_kind = 'environment';
CREATE UNIQUE INDEX agent_secrets_agent_path_uk
    ON agent_secrets (environment_id, agent_id, agent_fingerprint, path)
    WHERE deleted_at IS NULL AND owner_kind = 'agent';
CREATE UNIQUE INDEX agent_secrets_tool_path_uk ON agent_secrets (environment_id, tool_name, path)
    WHERE deleted_at IS NULL AND owner_kind = 'tool';

CREATE INDEX agent_secrets_environment_agent_idx
    ON agent_secrets (environment_id, agent_id, agent_fingerprint);
CREATE INDEX agent_secrets_environment_agent_type_idx ON agent_secrets (environment_id, agent_type);
CREATE INDEX agent_secrets_environment_tool_idx ON agent_secrets (environment_id, tool_name);
