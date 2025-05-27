ALTER TABLE project_policies ADD COLUMN delete_project BOOLEAN DEFAULT false;
ALTER TABLE project_policies ADD COLUMN view_project BOOLEAN DEFAULT false;
ALTER TABLE project_policies ADD COLUMN view_plugin_installations BOOLEAN DEFAULT false;
ALTER TABLE project_policies ADD COLUMN create_plugin_installation BOOLEAN DEFAULT false;
ALTER TABLE project_policies ADD COLUMN update_plugin_installation BOOLEAN DEFAULT false;
ALTER TABLE project_policies ADD COLUMN delete_plugin_installation BOOLEAN DEFAULT false;
