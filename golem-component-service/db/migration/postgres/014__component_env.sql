ALTER TABLE component_versions
    ADD COLUMN env JSONB NOT NULL
        DEFAULT '{}'::jsonb;