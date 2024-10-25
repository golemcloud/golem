CREATE TABLE components
(
    component_id        uuid    NOT NULL PRIMARY KEY,
    namespace           text    NOT NULL,
    name                text    NOT NULL
);

CREATE UNIQUE INDEX components_namespace_name_idx ON components (namespace, name);

CREATE TABLE component_versions
(
    component_id        uuid    NOT NULL REFERENCES components (component_id),
    version             bigint  NOT NULL,
    size                integer NOT NULL,
    created_at          timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    metadata            blob    NOT NULL,
    PRIMARY KEY (component_id, version)
);

CREATE TABLE component_initial_files
(
    component_id        uuid    NOT NULL,
    version             bigint  NOT NULL,
    file_path           text    NOT NULL,
    file_permission     integer NOT NULL,
    created_at          timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    PRIMARY KEY (component_id, version, file_path),
    FOREIGN KEY (component_id, version) REFERENCES component_versions (component_id, version)
);

CREATE INDEX component_initial_files_id_version ON component_initial_files (component_id, version);