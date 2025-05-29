CREATE TABLE project_plugin_installation
(
    installation_id uuid    NOT NULL PRIMARY KEY,
    plugin_name     text    NOT NULL,
    plugin_version  text    NOT NULL,
    priority        integer NOT NULL,
    parameters      blob    NOT NULL,
    project_id      uuid REFERENCES projects (project_id),
    account_id      character varying(100) NOT NULL
);
