CREATE TABLE installation
(
    installation_id INTEGER NOT NULL,
    golem_issuer    TEXT    NOT NULL,

    CONSTRAINT installation_pk
        PRIMARY KEY (installation_id),
    CONSTRAINT installation_single_row_check
        CHECK (installation_id = 1)
);
