ALTER TABLE accounts
    ADD COLUMN account_root_card_id UUID NULL REFERENCES cards(card_id) ON DELETE SET NULL;

CREATE UNIQUE INDEX accounts_account_root_card_id_uk
    ON accounts (account_root_card_id)
    WHERE account_root_card_id IS NOT NULL;
