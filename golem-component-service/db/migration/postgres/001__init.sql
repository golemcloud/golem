CREATE TABLE components
(
    component_id        uuid    NOT NULL PRIMARY KEY,
    namespace           text    NOT NULL,
    name                text    NOT NULL
);

CREATE INDEX components_namespace_id_idx ON components (namespace, component_id);

CREATE UNIQUE INDEX components_namespace_name_idx ON components (namespace, name);

CREATE TABLE component_versions
(
    component_id        uuid    NOT NULL REFERENCES components (component_id),
    version             bigint  NOT NULL,
    size                integer NOT NULL,
    created_at          timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP,
    user_component      text    NOT NULL,
    protected_component text    NOT NULL,
    protector_version   bigint,
    metadata            bytea   NOT NULL,
    PRIMARY KEY (component_id, version)
);
