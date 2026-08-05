ALTER TABLE plans
    ADD COLUMN max_disk_space_per_worker_ceiling NUMERIC NOT NULL DEFAULT 1073741824;

ALTER TABLE plans
    ADD COLUMN max_disk_space_per_worker_user_configurable BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE plans SET max_disk_space_per_worker_ceiling = max_disk_space_per_worker
WHERE max_disk_space_per_worker > 1073741824;

CREATE TABLE account_resource_overrides
(
    account_id     UUID      NOT NULL REFERENCES accounts,
    dimension      TEXT      NOT NULL,
    override_value NUMERIC   NOT NULL,
    reason         TEXT      NOT NULL,
    expires_at     TIMESTAMP,
    created_by     UUID      NOT NULL REFERENCES accounts,
    created_at     TIMESTAMP NOT NULL,

    CONSTRAINT account_resource_overrides_pk PRIMARY KEY (account_id, dimension)
);
