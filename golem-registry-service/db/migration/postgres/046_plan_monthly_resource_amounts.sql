ALTER TABLE plans
    ADD COLUMN monthly_compute_gcu NUMERIC NOT NULL DEFAULT 0,
    ADD COLUMN monthly_durable_storage_gb_month NUMERIC NOT NULL DEFAULT 0,
    ADD COLUMN monthly_ephemeral_storage_gb_month NUMERIC NOT NULL DEFAULT 0,
    DROP COLUMN monthly_memory_gb_seconds_ceiling,
    DROP COLUMN monthly_memory_gb_seconds_user_configurable;
