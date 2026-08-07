ALTER TABLE plans
    ADD COLUMN max_memory_per_worker_ceiling NUMERIC NOT NULL DEFAULT 1000000000000000000,
    ADD COLUMN max_memory_per_worker_user_configurable BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN monthly_memory_gb_seconds NUMERIC NOT NULL DEFAULT 1000000000000000000,
    ADD COLUMN monthly_memory_gb_seconds_ceiling NUMERIC NOT NULL DEFAULT 1000000000000000000,
    ADD COLUMN monthly_memory_gb_seconds_user_configurable BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE plans
SET max_memory_per_worker_ceiling = GREATEST(
    max_memory_per_worker_ceiling,
    max_memory_per_worker
);

INSERT INTO usage_types (usage_type, name) VALUES (12, 'MONTHLY_MEMORY_GB_SECONDS');
