ALTER TABLE plans
    DROP COLUMN IF EXISTS total_worker_count;

DELETE FROM account_usage_stats
WHERE usage_type = 0;

DELETE FROM usage_types
WHERE usage_type = 0;
