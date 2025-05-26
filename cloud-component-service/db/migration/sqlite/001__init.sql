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
