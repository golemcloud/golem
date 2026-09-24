ALTER TABLE deployment_tool_middleware_bindings
    ADD COLUMN config_keys_readable BLOB NOT NULL;

ALTER TABLE deployment_tool_middleware_bindings
    ADD COLUMN secret_keys_readable BLOB NOT NULL;

ALTER TABLE deployment_tool_middleware_bindings
    ADD COLUMN secret_keys_revealable BLOB NOT NULL;
