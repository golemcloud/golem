ALTER TABLE component_versions
    ADD COLUMN IF NOT EXISTS integer component_type NOT NULL DEFAULT 0;