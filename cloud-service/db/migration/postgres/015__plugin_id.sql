-- neither dev nor prod have data currently, save to change without migration
ALTER TABLE project_plugin_installation
    ADD COLUMN plugin_id uuid NOT NULL,
    DROP COLUMN plugin_name,
    DROP COLUMN plugin_version;
