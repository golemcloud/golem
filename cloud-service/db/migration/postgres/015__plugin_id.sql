
CREATE TABLE project_plugin_installation_copy
(
    installation_id   uuid    NOT NULL PRIMARY KEY,
    plugin_id            uuid    NOT NULL,
    priority          integer NOT NULL,
    parameters        blob    NOT NULL,
    component_id      uuid REFERENCES components (component_id),
    component_version bigint,

    FOREIGN KEY (plugin_id) REFERENCES plugins_copy (id)
);

INSERT INTO project_plugin_installation_copy
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
FROM project_plugin_installation;

DROP TABLE project_plugin_installation;
ALTER TABLE project_plugin_installation_copy RENAME TO project_plugin_installation;
