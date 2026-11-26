-- Audit records of authorization decisions wait here for the account audit export. A row leaves
-- only after the export acknowledges it; nothing reads this table as a history.
CREATE TABLE audit_staging
(
    event_id              UUID      NOT NULL,
    recorded_at           TIMESTAMP NOT NULL,
    surface               TEXT      NOT NULL,
    operation             TEXT      NOT NULL,
    outcome               TEXT      NOT NULL,
    target_account_id     UUID,
    target_environment_id UUID,
    record                TEXT      NOT NULL,

    CONSTRAINT audit_staging_pk
        PRIMARY KEY (event_id)
);
