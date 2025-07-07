ALTER TABLE component_versions
ADD COLUMN transformed_env JSONB;

UPDATE component_versions
SET transformed_env = env::JSONB,
       object_store_key = COALESCE(component_versions.object_store_key, component_versions.component_id::text || '#' || component_versions.version::text),
       transformed_object_store_key = COALESCE(component_versions.transformed_object_store_key, component_versions.component_id::text || '#' || component_versions.version::text);

ALTER TABLE component_versions
ALTER COLUMN transformed_env SET NOT NULL,
ALTER COLUMN object_store_key SET NOT NULL,
ALTER COLUMN transformed_object_store_key SET NOT NULL,
DROP COLUMN available;

CREATE TABLE transformed_component_files (
    component_id uuid NOT NULL,
    version bigint NOT NULL,
    file_key VARCHAR(255) NOT NULL,
    file_path VARCHAR(255) NOT NULL,
    file_permissions VARCHAR(255) NOT NULL,
    PRIMARY KEY (component_id, version, file_path),
    FOREIGN KEY (component_id, version) REFERENCES component_versions (component_id, version)
);

INSERT INTO transformed_component_files (component_id, version, file_key, file_path, file_permissions)
SELECT * FROM component_files;
