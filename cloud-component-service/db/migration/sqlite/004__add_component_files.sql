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
