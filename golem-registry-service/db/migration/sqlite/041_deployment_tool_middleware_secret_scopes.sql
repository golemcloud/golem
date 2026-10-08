ALTER TABLE deployment_tool_middleware_installations
    ADD COLUMN secret_keys_readable BLOB;

ALTER TABLE deployment_tool_middleware_installations
    ADD COLUMN secret_keys_revealable BLOB;
