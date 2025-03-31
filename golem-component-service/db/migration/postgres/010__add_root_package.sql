ALTER TABLE component_versions
    ADD COLUMN IF NOT EXISTS root_package_name text NOT NULL;
ALTER TABLE component_versions
    ADD COLUMN IF NOT EXISTS root_package_version text;