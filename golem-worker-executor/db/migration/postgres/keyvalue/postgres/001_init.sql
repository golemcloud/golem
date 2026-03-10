CREATE TABLE kv_storage (
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    value BYTEA NOT NULL,
    PRIMARY KEY (namespace, key)
);

CREATE TABLE set_storage (
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    value_hash BYTEA NOT NULL,
    value BYTEA NOT NULL,
    PRIMARY KEY (namespace, key, value_hash)
);

CREATE TABLE sorted_set_storage (
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    value_hash BYTEA NOT NULL,
    value BYTEA NOT NULL,
    score DOUBLE PRECISION NOT NULL,
    PRIMARY KEY (namespace, key, value_hash)
);

CREATE INDEX idx_sorted_set_storage_namespace_key_score
    ON sorted_set_storage (namespace, key, score);
