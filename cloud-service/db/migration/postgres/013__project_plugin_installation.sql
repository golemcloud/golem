
CREATE TABLE project_plugin_installation
(
    installation_id   uuid    NOT NULL PRIMARY KEY,
    plugin_name       text    NOT NULL,
    plugin_version    text    NOT NULL,
    priority          integer NOT NULL,
    parameters        bytea   NOT NULL,
    project_id        uuid    REFERENCES projects (project_id),
    account_id        VARCHAR(100) NOT NULL
);