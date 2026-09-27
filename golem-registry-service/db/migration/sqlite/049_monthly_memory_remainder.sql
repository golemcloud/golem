CREATE TABLE account_monthly_memory_remainders (
    account_id UUID NOT NULL,
    usage_key TEXT NOT NULL,
    byte_nanoseconds NUMERIC NOT NULL,
    updated_at TIMESTAMP NOT NULL,
    CONSTRAINT account_monthly_memory_remainders_pk PRIMARY KEY (account_id, usage_key),
    CONSTRAINT account_monthly_memory_remainders_account_id_fk FOREIGN KEY (account_id)
        REFERENCES accounts (account_id)
);
