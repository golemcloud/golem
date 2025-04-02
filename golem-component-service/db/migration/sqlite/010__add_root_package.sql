ALTER TABLE component_versions
    ADD COLUMN root_package_name text;
ALTER TABLE component_versions
    ADD COLUMN root_package_version text;