ALTER TABLE component_versions
    ADD COLUMN env TEXT NOT NULL DEFAULT '{}';