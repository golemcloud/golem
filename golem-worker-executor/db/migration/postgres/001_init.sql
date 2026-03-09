CREATE TABLE IF NOT EXISTS index_storage (
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    id BIGINT NOT NULL,
    value BYTEA NOT NULL,
    PRIMARY KEY (namespace, key, id)
);

CREATE INDEX IF NOT EXISTS idx_index_storage_ns_key ON index_storage (namespace, key);
