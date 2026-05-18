CREATE TABLE index_storage (
    namespace TEXT NOT NULL,
    key       TEXT NOT NULL,
    id        INTEGER NOT NULL,
    value     BLOB NOT NULL,
    PRIMARY KEY (namespace, key, id)
);

CREATE INDEX idx_key
    ON index_storage (namespace, key);
