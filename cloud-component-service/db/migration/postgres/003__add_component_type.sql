ALTER TABLE component_versions
    ADD COLUMN IF NOT EXISTS component_type integer NOT NULL DEFAULT 0;
