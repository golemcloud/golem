CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

ALTER TABLE plugins
    ADD COLUMN id uuid NOT NULL DEFAULT (uuid_generate_v4());

ALTER TABLE plugins
    ALTER COLUMN id DROP DEFAULT;

CREATE UNIQUE INDEX plugins_pkey_2 ON plugins (id);
CREATE UNIQUE INDEX plugins_name_version_unique ON plugins (name, version) WHERE (deleted IS FALSE);

ALTER TABLE component_plugin_installation
    ADD COLUMN plugin_id uuid,
    ADD CONSTRAINT component_plugin_installation_id_fkey FOREIGN KEY (plugin_id) REFERENCES plugins (id);

UPDATE component_plugin_installation
SET plugin_id = p.id
    FROM plugins AS p
WHERE p.name = plugin_name AND p.version = plugin_version;

ALTER TABLE component_plugin_installation ALTER COLUMN plugin_id SET NOT NULL;

ALTER TABLE component_plugin_installation
DROP CONSTRAINT IF EXISTS component_plugin_installation_plugin_name_plugin_version_fkey,
    DROP COLUMN plugin_name,
    DROP COLUMN plugin_version;

ALTER TABLE plugins
    ADD PRIMARY KEY USING INDEX plugins_pkey_2,
DROP CONSTRAINT IF EXISTS plugins_pkey;
