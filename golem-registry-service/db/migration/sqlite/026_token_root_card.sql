ALTER TABLE accounts
    ADD COLUMN token_root_card_id UUID NULL REFERENCES cards(card_id) ON DELETE SET NULL;

ALTER TABLE accounts
    ADD COLUMN token_root_card_epoch BIGINT;

UPDATE accounts SET token_root_card_epoch = 0;

CREATE INDEX accounts_token_root_card_id_idx ON accounts (token_root_card_id);
