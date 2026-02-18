ALTER TABLE component_revisions
    ADD original_wasi_config_vars JSONB;

ALTER TABLE component_revisions
    ADD wasi_config_vars JSONB;
