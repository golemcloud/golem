-- The shard epoch authorised to write each oplog. See the postgres migration of the same name.
CREATE TABLE oplog_metadata (
    namespace TEXT NOT NULL,
    key       TEXT NOT NULL,
    epoch     INTEGER NOT NULL,
    PRIMARY KEY (namespace, key)
);
