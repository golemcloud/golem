ALTER TABLE accounts
    ADD COLUMN token_root_card_id UUID NULL REFERENCES cards(card_id) ON DELETE SET NULL,
    ADD COLUMN token_root_card_epoch BIGINT;

UPDATE accounts SET token_root_card_epoch = 0;

ALTER TABLE accounts
    ALTER COLUMN token_root_card_epoch SET NOT NULL;

CREATE INDEX accounts_token_root_card_id_idx ON accounts (token_root_card_id);
