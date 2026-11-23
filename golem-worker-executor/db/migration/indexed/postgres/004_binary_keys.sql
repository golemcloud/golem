LOCK TABLE index_storage, indexed_key_epoch IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM index_storage) OR EXISTS (SELECT 1 FROM indexed_key_epoch) THEN
        RAISE EXCEPTION 'binary indexed storage requires empty tables; stop all writers and reset the indexed storage database before upgrading (no data migration is supported)';
    END IF;
END $$;

DROP TABLE index_storage, indexed_key_epoch;

CREATE TABLE index_storage (
    namespace TEXT NOT NULL,
    env UUID NOT NULL,
    agent UUID NOT NULL,
    id BIGINT NOT NULL,
    value BYTEA NOT NULL,
    PRIMARY KEY (namespace, env, agent, id)
);

ALTER TABLE index_storage SET (
    autovacuum_vacuum_scale_factor = 0.01,
    autovacuum_vacuum_threshold = 1024,
    autovacuum_analyze_scale_factor = 0.02,
    autovacuum_analyze_threshold = 1024,
    autovacuum_vacuum_cost_limit = 2000
);

CREATE TABLE indexed_key_epoch (
    namespace TEXT NOT NULL,
    env UUID NOT NULL,
    agent UUID NOT NULL,
    epoch BIGINT NOT NULL,
    PRIMARY KEY (namespace, env, agent)
);
