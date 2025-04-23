CREATE TABLE plugins_copy
(
    id   uuid    NOT NULL PRIMARY KEY,
    name                 text     NOT NULL,
    version              text     NOT NULL,
    description          text     NOT NULL,
    icon                 blob     NOT NULL,
    homepage             text     NOT NULL,
    plugin_type          smallint NOT NULL,
    scope_component_id   uuid REFERENCES components (component_id),
    provided_wit_package text,
    json_schema          text,
    validate_url         text,
    transform_url        text,
    component_id         uuid REFERENCES components (component_id),
    component_version    bigint,
    deleted              boolean  NOT NULL DEFAULT FALSE,
    blob_storage_key text
);

CREATE UNIQUE INDEX IF NOT EXISTS plugins_name_version_unique ON plugins_copy (name, version) WHERE (deleted IS FALSE);

INSERT INTO plugins_copy
(
    id,
    name,
    version,
    description,
    icon,
    homepage,
    plugin_type,
    scope_component_id,
    provided_wit_package,
    json_schema,
    validate_url,
    transform_url,
    component_id,
    component_version,
    deleted,
    blob_storage_key
)
SELECT
    -- https://stackoverflow.com/a/41649754
    (lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))),2) || '-' || substr('89ab',abs(random()) % 4 + 1, 1) || substr(lower(hex(randomblob(2))),2) || '-' || lower(hex(randomblob(6)))),
    name,
    version,
    description,
    icon,
    homepage,
    plugin_type,
    scope_component_id,
    provided_wit_package,
    json_schema,
    validate_url,
    transform_url,
    component_id,
    component_version,
    deleted,
    blob_storage_key
FROM plugins;

CREATE TABLE component_plugin_installation_copy
(
    installation_id   uuid    NOT NULL PRIMARY KEY,
    plugin_id            uuid    NOT NULL,
    priority          integer NOT NULL,
    parameters        blob    NOT NULL,
    component_id      uuid REFERENCES components (component_id),
    component_version bigint,

    FOREIGN KEY (plugin_id) REFERENCES plugins_copy (id)
);

INSERT INTO component_plugin_installation_copy
(
    installation_id,
    plugin_id,
    priority,
    parameters,
    component_id,
    component_version
)
SELECT
    installation_id,
    (SELECT p.id FROM plugins_copy p WHERE p.name = plugin_name AND p.version = plugin_version),
    priority,
    parameters,
    component_id,
    component_version
FROM component_plugin_installation;

DROP TABLE component_plugin_installation;
ALTER TABLE component_plugin_installation_copy RENAME TO component_plugin_installation;

DROP TABLE plugins;
ALTER TABLE plugins_copy RENAME TO plugins;
