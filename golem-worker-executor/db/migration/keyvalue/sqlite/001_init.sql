CREATE TABLE kv_storage (
    key       TEXT NOT NULL,
    value     BLOB NOT NULL,
    namespace TEXT NOT NULL,
    PRIMARY KEY (key, namespace)
);

CREATE TABLE set_storage (
    key       TEXT NOT NULL,
    value     BLOB NOT NULL,
    namespace TEXT NOT NULL,
    PRIMARY KEY (key, value, namespace)
);

CREATE INDEX idx_set_storage_key_namespace
    ON set_storage (key, namespace);

CREATE TABLE sorted_set_storage (
    key       TEXT NOT NULL,
    value     BLOB NOT NULL,
    namespace TEXT NOT NULL,
    score     REAL NOT NULL,
    PRIMARY KEY (key, value, namespace)
);

CREATE INDEX idx_sorted_set_storage_key_namespace
    ON sorted_set_storage (key, namespace);

CREATE INDEX idx_sorted_set_storage_score
    ON sorted_set_storage (score);
