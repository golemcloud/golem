ALTER TABLE component_versions RENAME TO component_versions_temp;
CREATE TABLE component_versions
(
    component_id        uuid    NOT NULL REFERENCES components (component_id),
    version             bigint  NOT NULL,
    size                integer NOT NULL,
    created_at          timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    metadata            blob    NOT NULL,
    component_type integer NOT NULL DEFAULT 0,
    object_store_key text NOT NULL,
    transformed_object_store_key text NOT NULL,
    root_package_name text,
    root_package_version text,
    env TEXT NOT NULL,
    transformed_env TEXT NOT NULL,
    PRIMARY KEY (component_id, version)
);

INSERT INTO component_versions (
    component_id,
    version,
    size,
    created_at,
    metadata,
    component_type,
    object_store_key,
    transformed_object_store_key,
    root_package_name,
    root_package_version,
    env,
    transformed_env
)
SELECT
    component_id,
    version, size,
    created_at,
    metadata,
    component_type,
    COALESCE(object_store_key, component_id || '#' || version) as object_store_key,
    COALESCE(transformed_object_store_key, component_id || '#' || version) as transformed_object_store_key,
    root_package_name,
    root_package_version,
    env,
    env as transformed_env
FROM component_versions_temp;

ALTER TABLE component_files RENAME TO component_files_temp;
CREATE TABLE component_files
(
    component_id uuid NOT NULL,
    version bigint NOT NULL,
    file_key VARCHAR(255) NOT NULL,
    file_path VARCHAR(255) NOT NULL,
    file_permissions VARCHAR(255) NOT NULL,
    PRIMARY KEY (component_id, version, file_path),
    FOREIGN KEY (component_id, version) REFERENCES component_versions (component_id, version)
);
INSERT INTO component_files SELECT * FROM component_files_temp;
DROP TABLE component_files_temp;

DROP TABLE component_versions_temp;

CREATE TABLE transformed_component_files (
    component_id uuid NOT NULL,
    version bigint NOT NULL,
    file_key VARCHAR(255) NOT NULL,
    file_path VARCHAR(255) NOT NULL,
    file_permissions VARCHAR(255) NOT NULL,
    PRIMARY KEY (component_id, version, file_path),
    FOREIGN KEY (component_id, version) REFERENCES component_versions (component_id, version)
);

INSERT INTO transformed_component_files SELECT * FROM component_files;
