CREATE TABLE IF NOT EXISTS account_components (
    account_id VARCHAR(100) NOT NULL REFERENCES accounts(id),
    counter INTEGER NOT NULL,
    PRIMARY KEY (account_id)
);

CREATE TABLE IF NOT EXISTS account_used_storage (
    account_id VARCHAR(100) NOT NULL REFERENCES accounts(id),
    counter BIGINT NOT NULL,
    PRIMARY KEY (account_id)
);