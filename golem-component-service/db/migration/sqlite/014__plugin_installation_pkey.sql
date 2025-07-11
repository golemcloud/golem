alter table component_plugin_installation rename to component_plugin_installation_temp;

CREATE TABLE component_plugin_installation
(
    installation_id   uuid    NOT NULL,
    plugin_id            uuid    NOT NULL,
    priority          integer NOT NULL,
    parameters        blob    NOT NULL,
    component_id      uuid REFERENCES components (component_id) NOT NULL,
    component_version bigint NOT NULL,
    account_id       text NOT NULL,

    PRIMARY KEY (installation_id, component_id, component_version)
    FOREIGN KEY (account_id, plugin_id) REFERENCES "plugins" (account_id, id)
);

INSERT INTO component_plugin_installation SELECT * FROM component_plugin_installation_temp;
DROP TABLE component_plugin_installation_temp;
