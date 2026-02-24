ALTER TABLE component_revisions
    ADD original_config_vars JSONB;

ALTER TABLE component_revisions
    ADD config_vars JSONB;
