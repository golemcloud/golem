CREATE TABLE whitelist (
    email TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_by VARCHAR(100) NOT NULL REFERENCES accounts(id),
    PRIMARY KEY (email)
);

CREATE TABLE account_creation_attempts (
    oauth2_provider TEXT NOT NULL,
    external_id TEXT NOT NULL,
    name TEXT NOT NULL,
    email TEXT NOT NULL,
    all_emails jsonb NOT NULL,
    first_attempt TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    latest_attempt TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    attempts_count INTEGER NOT NULL,
    PRIMARY KEY (oauth2_provider, external_id)
);

CREATE TABLE configs (
    id TEXT NOT NULL,
    config jsonb NOT NULL,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_by VARCHAR(100) NOT NULL REFERENCES accounts(id),
    PRIMARY KEY (id)
);
