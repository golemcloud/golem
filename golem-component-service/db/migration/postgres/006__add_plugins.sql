CREATE TABLE plugins
(
    name                 text     NOT NULL,
    version              text     NOT NULL,
    description          text     NOT NULL,
    icon                 bytea    NOT NULL,
    homepage             text     NOT NULL,
    plugin_type          smallint NOT NULL,
    scope_component_id   uuid REFERENCES components (component_id),
    scope_project_id     uuid,
    account_id           VARCHAR(100) NOT NULL,
    provided_wit_package text,
    json_schema          text,
    validate_url         text,
    transform_url        text,
    component_id         uuid REFERENCES components (component_id),
    component_version    bigint,
    deleted              boolean  NOT NULL DEFAULT FALSE,

    PRIMARY KEY (account_id, name, version)
);

CREATE TABLE component_plugin_installation
(
    installation_id   uuid    NOT NULL PRIMARY KEY,
    plugin_name       text    NOT NULL,
    plugin_version    text    NOT NULL,
    priority          integer NOT NULL,
    parameters        bytea   NOT NULL,
    component_id      uuid REFERENCES components (component_id),
    component_version bigint,
    account_id        VARCHAR(100) NOT NULL,

    FOREIGN KEY (account_id, plugin_name, plugin_version) REFERENCES plugins (account_id, name, version)
);