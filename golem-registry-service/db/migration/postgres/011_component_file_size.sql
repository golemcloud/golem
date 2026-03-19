ALTER TABLE component_files ADD COLUMN file_size BIGINT NOT NULL DEFAULT 0;
ALTER TABLE original_component_files ADD COLUMN file_size BIGINT NOT NULL DEFAULT 0;
