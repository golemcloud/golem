ALTER TABLE component_revisions
    ADD original_config_vars JSONB NOT NULL;

ALTER TABLE component_revisions
    ADD config_vars JSONB NOT NULL;
