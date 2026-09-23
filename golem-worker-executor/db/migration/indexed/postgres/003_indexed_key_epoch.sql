-- The writer generation recorded for a key in `index_storage`.
--
-- One row per key: a monotonic epoch, and the writer that recorded it. An append that asserts an
-- epoch is checked against this row inside the same transaction as its insert, so a writer holding
-- a stale epoch is refused without leaving anything behind. A missing row refuses too: the row is
-- written before the first entry and removed before the entries are.
--
-- The epoch alone cannot separate two writers presenting the same number, so the row names the
-- writer as well: the one that recorded it may go on writing at that epoch, and any other is
-- refused.
CREATE TABLE indexed_key_epoch (
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    epoch BIGINT NOT NULL,
    writer TEXT NOT NULL,
    PRIMARY KEY (namespace, key)
);
