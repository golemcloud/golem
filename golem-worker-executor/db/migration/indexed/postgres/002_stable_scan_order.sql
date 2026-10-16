DO $$
BEGIN
    IF current_setting('server_encoding') <> 'UTF8' THEN
        RAISE EXCEPTION 'indexed storage requires UTF8 server encoding';
    END IF;
END $$;

ALTER TABLE index_storage
    ALTER COLUMN key TYPE TEXT COLLATE "C";

ANALYZE index_storage;
