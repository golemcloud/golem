CREATE TABLE IF NOT EXISTS index_storage (
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    id BIGINT NOT NULL,
    value BYTEA NOT NULL,
    PRIMARY KEY (namespace, key, id)
);

CREATE INDEX IF NOT EXISTS idx_index_storage_ns_key ON index_storage (namespace, key);

ALTER TABLE index_storage SET (
    autovacuum_vacuum_scale_factor = 0.01,
    autovacuum_vacuum_threshold = 1024,
    autovacuum_analyze_scale_factor = 0.02,
    autovacuum_analyze_threshold = 1024,
    autovacuum_vacuum_cost_limit = 2000
);
