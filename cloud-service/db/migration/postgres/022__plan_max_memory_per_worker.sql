ALTER TABLE plans
ADD COLUMN max_memory_per_worker INTEGER DEFAULT (1024 * 1024 * 1024);

-- backfill existing rows
UPDATE plans
SET max_memory_per_worker = 1024 * 1024 * 1024
WHERE max_memory_per_worker IS NULL;

-- enforce NOT NULL
ALTER TABLE plans
ALTER COLUMN max_memory_per_worker SET NOT NULL;

-- drop the default so new inserts must explicitly provide a value
ALTER TABLE plans
ALTER COLUMN max_memory_per_worker DROP DEFAULT;
