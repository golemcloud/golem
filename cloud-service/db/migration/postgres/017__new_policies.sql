ALTER TABLE project_policies
    ADD COLUMN delete_project BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN view_project BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN view_plugin_installations BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN create_plugin_installation BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN update_plugin_installation BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN delete_plugin_installation BOOLEAN NOT NULL DEFAULT false;
