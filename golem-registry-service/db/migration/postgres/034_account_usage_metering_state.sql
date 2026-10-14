CREATE TABLE account_usage_metering_state
(
    account_id        UUID      NOT NULL,
    usage_key         TEXT      NOT NULL,
    compute_enabled   BOOLEAN   NOT NULL,
    memory_enabled    BOOLEAN   NOT NULL,
    filesystem_enabled BOOLEAN  NOT NULL,
    updated_at        TIMESTAMP NOT NULL,
    CONSTRAINT account_usage_metering_state_pk
        PRIMARY KEY (account_id, usage_key),
    CONSTRAINT account_usage_metering_state_accounts_fk
        FOREIGN KEY (account_id) REFERENCES accounts
);
