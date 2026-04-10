CREATE TABLE retry_policies
(
    retry_policy_id     UUID      NOT NULL,

    environment_id      UUID      NOT NULL,
    name                TEXT      NOT NULL,

    created_at          TIMESTAMP NOT NULL,
    updated_at          TIMESTAMP NOT NULL,
    deleted_at          TIMESTAMP,
    modified_by         UUID      NOT NULL,

    current_revision_id BIGINT    NOT NULL,

    CONSTRAINT retry_policies_pk
        PRIMARY KEY (retry_policy_id),
    CONSTRAINT retry_policies_environments_fk
        FOREIGN KEY (environment_id) REFERENCES environments
);

CREATE UNIQUE INDEX retry_policies_environment_name_uk ON retry_policies (environment_id, name)
    WHERE deleted_at IS NULL;

CREATE TABLE retry_policy_revisions
(
    retry_policy_id     UUID      NOT NULL,
    revision_id         BIGINT    NOT NULL,

    priority            BIGINT    NOT NULL,
    predicate_json      TEXT      NOT NULL,
    policy_json         TEXT      NOT NULL,

    created_at          TIMESTAMP NOT NULL,
    created_by          UUID      NOT NULL,
    deleted             BOOLEAN   NOT NULL,

    CONSTRAINT retry_policy_revisions_pk
        PRIMARY KEY (retry_policy_id, revision_id),
    CONSTRAINT retry_policy_revisions_retry_policies_fk
        FOREIGN KEY (retry_policy_id) REFERENCES retry_policies
);
