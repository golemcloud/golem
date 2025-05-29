ALTER TABLE component_versions
    ADD COLUMN IF NOT EXISTS transformed_object_store_key text;
