ALTER TABLE plans
    ADD COLUMN overage_eligible BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE account_monthly_usage_modes (
    account_id UUID NOT NULL,
    mode TEXT NOT NULL,
    revision NUMERIC NOT NULL,
    changed_by UUID NOT NULL,
    changed_at TIMESTAMP NOT NULL,
    CONSTRAINT account_monthly_usage_modes_pk PRIMARY KEY (account_id),
    CONSTRAINT account_monthly_usage_modes_account_id_fk FOREIGN KEY (account_id)
        REFERENCES accounts (account_id),
    CONSTRAINT account_monthly_usage_modes_mode_check
        CHECK (mode IN ('hard_limit', 'allow_overage'))
);

CREATE TABLE account_monthly_usage_mode_transitions (
    transition_id UUID NOT NULL,
    account_id UUID NOT NULL,
    revision NUMERIC NOT NULL,
    actor_account_id UUID NOT NULL,
    source TEXT NOT NULL,
    changed_at TIMESTAMP NOT NULL,
    previous_mode TEXT NOT NULL,
    new_mode TEXT NOT NULL,
    period_year INTEGER NOT NULL,
    period_month INTEGER NOT NULL,
    compute_fuel NUMERIC NOT NULL,
    memory_gb_seconds NUMERIC NOT NULL,
    durable_storage_byte_seconds NUMERIC NOT NULL,
    ephemeral_storage_byte_seconds NUMERIC NOT NULL,
    CONSTRAINT account_monthly_usage_mode_transitions_pk PRIMARY KEY (transition_id),
    CONSTRAINT account_monthly_usage_mode_transitions_account_id_fk FOREIGN KEY (account_id)
        REFERENCES accounts (account_id),
    CONSTRAINT account_monthly_usage_mode_transitions_source_check
        CHECK (source IN ('owner', 'administrator', 'plan_eligibility_removed', 'ineligible_plan_assigned')),
    CONSTRAINT account_monthly_usage_mode_transitions_previous_mode_check
        CHECK (previous_mode IN ('hard_limit', 'allow_overage')),
    CONSTRAINT account_monthly_usage_mode_transitions_new_mode_check
        CHECK (new_mode IN ('hard_limit', 'allow_overage')),
    CONSTRAINT account_monthly_usage_mode_transitions_consent_source_check
        CHECK (new_mode <> 'allow_overage' OR source = 'owner'),
    CONSTRAINT account_monthly_usage_mode_transitions_consent_actor_check
        CHECK (new_mode <> 'allow_overage' OR actor_account_id = account_id),
    CONSTRAINT account_monthly_usage_mode_transitions_period_month_check
        CHECK (period_month BETWEEN 1 AND 12)
);

CREATE INDEX account_monthly_usage_mode_transitions_account_time_idx
    ON account_monthly_usage_mode_transitions (account_id, changed_at DESC);

CREATE UNIQUE INDEX account_monthly_usage_mode_transitions_account_revision_uk
    ON account_monthly_usage_mode_transitions (account_id, revision);

CREATE TABLE account_monthly_usage_mode_attribution (
    usage_update_id UUID NOT NULL,
    account_id UUID NOT NULL,
    revision NUMERIC NOT NULL,
    usage_key TEXT NOT NULL,
    compute_fuel_delta BIGINT NOT NULL,
    memory_gb_seconds_delta BIGINT NOT NULL,
    durable_storage_byte_seconds_delta BIGINT NOT NULL,
    ephemeral_storage_byte_seconds_delta BIGINT NOT NULL,
    memory_byte_nanoseconds_remainder NUMERIC NOT NULL,
    durable_storage_byte_nanoseconds_remainder NUMERIC NOT NULL,
    ephemeral_storage_byte_nanoseconds_remainder NUMERIC NOT NULL,
    recorded_at TIMESTAMP NOT NULL,
    CONSTRAINT account_monthly_usage_mode_attribution_pk PRIMARY KEY (usage_update_id),
    CONSTRAINT account_monthly_usage_mode_attribution_account_id_fk FOREIGN KEY (account_id)
        REFERENCES accounts (account_id)
);

CREATE INDEX account_monthly_usage_mode_attribution_account_revision_idx
    ON account_monthly_usage_mode_attribution (account_id, revision, usage_key);
