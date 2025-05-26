ALTER TABLE component_versions
    ADD COLUMN IF NOT EXISTS available boolean NOT NULL DEFAULT true;
