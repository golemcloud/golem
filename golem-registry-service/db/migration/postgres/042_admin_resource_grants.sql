ALTER TABLE account_resource_overrides
    DROP CONSTRAINT account_resource_overrides_pk;

ALTER TABLE account_resource_overrides
    ADD COLUMN source TEXT;

UPDATE account_resource_overrides
SET source = 'self_service';

ALTER TABLE account_resource_overrides
    ALTER COLUMN source SET NOT NULL;

ALTER TABLE account_resource_overrides
    ADD CONSTRAINT account_resource_overrides_pk PRIMARY KEY (account_id, dimension, source),
    ADD CONSTRAINT account_resource_overrides_dimension_check
        CHECK (dimension IN (
            'monthly_compute_gcu',
            'monthly_memory_gb_seconds',
            'monthly_durable_storage_gb_month',
            'monthly_ephemeral_storage_gb_month',
            'max_disk_space_per_worker',
            'max_memory_per_worker'
        )),
    ADD CONSTRAINT account_resource_overrides_source_check
        CHECK (source IN ('self_service', 'admin_grant')),
    ADD CONSTRAINT account_resource_overrides_source_reason_check
        CHECK (
            (source = 'self_service' AND reason IN ('user_self_serve', 'downgrade_clamp'))
            OR (source = 'admin_grant' AND reason IN ('promotional', 'support'))
        );

CREATE TABLE account_resource_override_events (
    event_sequence BIGSERIAL PRIMARY KEY,
    event_id UUID NOT NULL UNIQUE,
    account_id UUID NOT NULL,
    dimension TEXT NOT NULL,
    event_type TEXT NOT NULL,
    reason TEXT NOT NULL,
    actor_account_id UUID NOT NULL,
    changed_at TIMESTAMP NOT NULL,
    old_value NUMERIC NOT NULL,
    new_value NUMERIC NOT NULL,
    expires_at TIMESTAMP,
    CONSTRAINT account_resource_override_events_account_id_fk FOREIGN KEY (account_id)
        REFERENCES accounts (account_id),
    CONSTRAINT account_resource_override_events_dimension_check
        CHECK (dimension IN (
            'monthly_compute_gcu',
            'monthly_memory_gb_seconds',
            'monthly_durable_storage_gb_month',
            'monthly_ephemeral_storage_gb_month',
            'max_disk_space_per_worker',
            'max_memory_per_worker'
        )),
    CONSTRAINT account_resource_override_events_type_check
        CHECK (event_type IN ('override_granted', 'override_cleared', 'override_expired')),
    CONSTRAINT account_resource_override_events_reason_check
        CHECK (reason IN ('promotional', 'support'))
);

CREATE INDEX account_resource_override_events_account_time_idx
    ON account_resource_override_events (account_id, changed_at DESC, event_sequence DESC);

CREATE INDEX account_resource_overrides_expiry_idx
    ON account_resource_overrides (expires_at)
    WHERE source = 'admin_grant' AND expires_at IS NOT NULL;
