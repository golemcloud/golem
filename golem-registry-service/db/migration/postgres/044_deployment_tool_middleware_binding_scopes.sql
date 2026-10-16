ALTER TABLE deployment_tool_middleware_bindings
    ADD COLUMN config_keys_readable BYTEA NOT NULL,
    ADD COLUMN secret_keys_readable BYTEA NOT NULL,
    ADD COLUMN secret_keys_revealable BYTEA NOT NULL;
