-- The writer generation recorded for a key in `index_storage`.
--
-- One row per key: a monotonic epoch. An append that asserts an epoch is checked against this row
-- inside the same transaction as its insert, so a writer holding a stale epoch is refused without
-- leaving anything behind. A missing row refuses too: the row is written before the first entry
-- and removed before the entries are.
CREATE TABLE indexed_key_epoch (
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    epoch BIGINT NOT NULL,
    PRIMARY KEY (namespace, key)
);
