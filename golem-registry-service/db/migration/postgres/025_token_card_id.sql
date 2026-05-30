ALTER TABLE tokens ADD COLUMN card_id UUID NULL REFERENCES cards(card_id) ON DELETE SET NULL;

CREATE INDEX tokens_card_id_idx ON tokens (card_id);
