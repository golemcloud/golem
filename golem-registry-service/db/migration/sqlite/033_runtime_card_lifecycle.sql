CREATE TABLE runtime_card_lifecycle
(
    card_id UUID    NOT NULL,
    revoked BOOLEAN NOT NULL,

    CONSTRAINT runtime_card_lifecycle_pk
        PRIMARY KEY (card_id)
);
