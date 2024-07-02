CREATE TABLE components
(
    namespace           text    NOT NULL,
    component_id        uuid    NOT NULL PRIMARY KEY,
    name                text    NOT NULL,
    UNIQUE (namespace, name)
);

CREATE INDEX components_namespace_id_idx ON components (namespace, component_id);

CREATE TABLE component_versions
(
    component_id        uuid    NOT NULL,
    version             bigint  NOT NULL,
    size                integer NOT NULL,
    created_at          timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    user_component      text    NOT NULL,
    protected_component text    NOT NULL,
    protector_version   bigint,
    metadata            blob    NOT NULL,
    PRIMARY KEY (component_id, version),
    FOREIGN KEY (component_id) REFERENCES components (component_id)
);

