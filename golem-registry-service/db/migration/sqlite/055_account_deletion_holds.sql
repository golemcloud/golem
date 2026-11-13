CREATE TABLE account_deletion_holds (
    account_id        UUID      NOT NULL,
    holder_account_id UUID      NOT NULL,
    reason            TEXT      NOT NULL,
    created_at        TIMESTAMP NOT NULL,
    updated_at        TIMESTAMP NOT NULL,
    CONSTRAINT account_deletion_holds_pk PRIMARY KEY (account_id, holder_account_id),
    CONSTRAINT account_deletion_holds_account_id_fk FOREIGN KEY (account_id)
        REFERENCES accounts (account_id),
    CONSTRAINT account_deletion_holds_holder_account_id_fk FOREIGN KEY (holder_account_id)
        REFERENCES accounts (account_id)
);
