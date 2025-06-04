ALTER TABLE plugins
    ADD COLUMN IF NOT EXISTS blob_storage_key text;
