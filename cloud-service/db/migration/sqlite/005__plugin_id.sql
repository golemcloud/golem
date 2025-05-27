ALTER TABLE project_plugin_installation
    ADD COLUMN plugin_id uuid NOT NULL;

ALTER TABLE project_plugin_installation
    DROP COLUMN plugin_name;

ALTER TABLE project_plugin_installation
    DROP COLUMN plugin_version;
