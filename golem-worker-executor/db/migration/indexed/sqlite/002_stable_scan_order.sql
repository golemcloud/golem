CREATE TEMPORARY TABLE indexed_storage_encoding_check (
    encoding TEXT NOT NULL CHECK (encoding = 'UTF-8')
);

INSERT INTO indexed_storage_encoding_check (encoding)
SELECT encoding FROM pragma_encoding();

DROP TABLE indexed_storage_encoding_check;
