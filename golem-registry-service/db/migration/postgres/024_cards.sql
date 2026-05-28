CREATE TABLE cards
(
    card_id        UUID      NOT NULL,
    data           BYTEA     NOT NULL,
    created_at     TIMESTAMP NOT NULL,
    expires_at     TIMESTAMP,
    system_card    BOOLEAN   NOT NULL,
    managed_by     BYTEA,

    CONSTRAINT cards_pk
        PRIMARY KEY (card_id)
);

CREATE TABLE card_parents
(
    card_id   UUID NOT NULL,
    parent_id UUID NOT NULL,

    CONSTRAINT card_parents_pk
        PRIMARY KEY (card_id, parent_id),
    CONSTRAINT card_parents_card_fk
        FOREIGN KEY (card_id) REFERENCES cards (card_id) ON DELETE CASCADE,
    CONSTRAINT card_parents_parent_fk
        FOREIGN KEY (parent_id) REFERENCES cards (card_id) ON DELETE CASCADE
);

CREATE INDEX card_parents_parent_id_idx
    ON card_parents (parent_id);

ALTER TABLE registry_change_events
    ADD card_ids BYTEA;
