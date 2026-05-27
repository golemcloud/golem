CREATE TABLE permission_shares
(
    permission_share_id UUID      NOT NULL,
    owner_account_id    UUID      NOT NULL,
    target_account_id   UUID      NOT NULL,
    name                TEXT      NOT NULL,
    current_card_id     UUID,

    created_at          TIMESTAMP NOT NULL,
    updated_at          TIMESTAMP NOT NULL,
    deleted_at          TIMESTAMP,
    modified_by         UUID      NOT NULL,

    current_revision_id BIGINT    NOT NULL,

    CONSTRAINT permission_shares_pk
        PRIMARY KEY (permission_share_id),
    CONSTRAINT permission_shares_owner_accounts_fk
        FOREIGN KEY (owner_account_id) REFERENCES accounts (account_id),
    CONSTRAINT permission_shares_target_accounts_fk
        FOREIGN KEY (target_account_id) REFERENCES accounts (account_id),
    CONSTRAINT permission_shares_current_cards_fk
        FOREIGN KEY (current_card_id) REFERENCES cards (card_id) ON DELETE SET NULL
);

CREATE UNIQUE INDEX permission_shares_owner_name_uk
    ON permission_shares (owner_account_id, name)
    WHERE deleted_at IS NULL;

CREATE UNIQUE INDEX permission_shares_current_card_id_uk
    ON permission_shares (current_card_id)
    WHERE current_card_id IS NOT NULL;

CREATE INDEX permission_shares_owner_account_id_idx
    ON permission_shares (owner_account_id);

CREATE INDEX permission_shares_target_account_id_idx
    ON permission_shares (target_account_id);

CREATE INDEX permission_shares_deleted_at_idx
    ON permission_shares (deleted_at);

CREATE TABLE permission_share_revisions
(
    permission_share_id UUID      NOT NULL,
    revision_id         BIGINT    NOT NULL,

    name                TEXT      NOT NULL,
    card_id             UUID,
    data                BLOB      NOT NULL,

    created_at          TIMESTAMP NOT NULL,
    created_by          UUID      NOT NULL,
    deleted             BOOLEAN   NOT NULL,

    CONSTRAINT permission_share_revisions_pk
        PRIMARY KEY (permission_share_id, revision_id),
    CONSTRAINT permission_share_revisions_permission_shares_fk
        FOREIGN KEY (permission_share_id) REFERENCES permission_shares,
    CONSTRAINT permission_share_revisions_cards_fk
        FOREIGN KEY (card_id) REFERENCES cards (card_id) ON DELETE SET NULL
);

CREATE INDEX permission_share_revisions_name_idx
    ON permission_share_revisions (permission_share_id, name);
