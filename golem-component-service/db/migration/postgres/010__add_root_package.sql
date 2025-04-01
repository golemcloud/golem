ALTER TABLE component_versions
    ADD COLUMN IF NOT EXISTS root_package_name text;
ALTER TABLE component_versions
    ADD COLUMN IF NOT EXISTS root_package_version text;