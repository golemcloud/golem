ALTER TABLE environments
    ADD environment_default_card_id UUID;

CREATE INDEX environments_environment_default_card_id_idx
    ON environments (environment_default_card_id);
