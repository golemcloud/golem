ALTER TABLE project_policies
    ADD COLUMN batch_update_plugin_installations BOOLEAN NOT NULL DEFAULT false;
