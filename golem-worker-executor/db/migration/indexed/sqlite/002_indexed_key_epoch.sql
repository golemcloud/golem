-- The writer generation recorded for a key in `index_storage`. See the postgres migration of the
-- same name.
CREATE TABLE indexed_key_epoch (
    namespace TEXT NOT NULL,
    key       TEXT NOT NULL,
    epoch     INTEGER NOT NULL,
    writer    TEXT NOT NULL,
    PRIMARY KEY (namespace, key)
);
