ALTER TABLE agent_secrets
    ADD COLUMN owner_kind TEXT NOT NULL DEFAULT 'environment',
    ADD COLUMN agent_id UUID,
    ADD COLUMN agent_fingerprint UUID,
    ADD COLUMN agent_type BIGINT,
    ADD COLUMN tool_name TEXT;

ALTER TABLE agent_secrets ALTER COLUMN owner_kind DROP DEFAULT;

ALTER TABLE agent_secrets
    ADD CONSTRAINT agent_secrets_owner_kind_check
        CHECK (owner_kind IN ('environment', 'agent', 'tool')),
    ADD CONSTRAINT agent_secrets_owner_fields_check
        CHECK (
            (owner_kind = 'environment' AND agent_id IS NULL AND agent_fingerprint IS NULL
                AND agent_type IS NULL AND tool_name IS NULL)
            OR (owner_kind = 'agent' AND agent_id IS NOT NULL AND agent_fingerprint IS NOT NULL
                AND agent_type IS NOT NULL AND tool_name IS NULL)
            OR (owner_kind = 'tool' AND agent_id IS NULL AND agent_fingerprint IS NULL
                AND agent_type IS NULL AND tool_name IS NOT NULL)
        ),
    ADD CONSTRAINT agent_secrets_agent_type_check
        CHECK (agent_type BETWEEN 1 AND 4294967295);

ALTER TABLE agent_secret_revisions ADD COLUMN destinations JSONB NOT NULL DEFAULT '[]';
ALTER TABLE agent_secret_revisions ALTER COLUMN destinations DROP DEFAULT;

DROP INDEX agent_secrets_environment_path_uk;
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
