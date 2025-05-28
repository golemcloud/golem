ALTER TABLE component_versions
    ADD COLUMN IF NOT EXISTS object_store_key text;
