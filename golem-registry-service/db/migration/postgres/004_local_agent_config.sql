ALTER TABLE component_revisions
    ADD local_agent_config BYTEA NOT NULL;

ALTER TABLE component_revisions
    DROP COLUMN original_env;

ALTER TABLE component_revisions
    DROP COLUMN original_config_vars;

ALTER TABLE component_revisions
    DROP COLUMN transformed_object_store_key;

ALTER TABLE component_revisions
    DROP COLUMN version;

DROP TABLE original_component_files;
