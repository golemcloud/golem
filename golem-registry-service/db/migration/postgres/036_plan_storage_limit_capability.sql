ALTER TABLE plans
    ADD COLUMN max_disk_space_per_worker_enabled BOOLEAN NOT NULL DEFAULT FALSE;

DELETE FROM account_resource_overrides
WHERE dimension = 'max_disk_space_per_worker';
