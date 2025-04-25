ALTER TABLE project_plugin_installation
    ADD COLUMN plugin_id uuid,
    ADD CONSTRAINT project_plugin_installation_id_fkey FOREIGN KEY (plugin_id) REFERENCES plugins (id);

UPDATE project_plugin_installation
SET plugin_id = p.id
    FROM plugins AS p
WHERE p.name = plugin_name AND p.version = plugin_version;

ALTER TABLE project_plugin_installation ALTER COLUMN plugin_id SET NOT NULL;

ALTER TABLE project_plugin_installation
DROP CONSTRAINT IF EXISTS project_plugin_installation_plugin_name_plugin_version_fkey,
    DROP COLUMN plugin_name,
    DROP COLUMN plugin_version;
