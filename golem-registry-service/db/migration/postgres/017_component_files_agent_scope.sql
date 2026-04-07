DELETE FROM component_files;

ALTER TABLE component_files
    ADD COLUMN agent_type_name TEXT NOT NULL;

ALTER TABLE component_files
    DROP CONSTRAINT component_files_pk;

ALTER TABLE component_files
    ADD CONSTRAINT component_files_pk
        PRIMARY KEY (component_id, revision_id, agent_type_name, file_path);
