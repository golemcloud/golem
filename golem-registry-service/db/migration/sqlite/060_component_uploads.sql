CREATE TABLE component_uploads (
    object_key TEXT NOT NULL,
    account_id UUID NOT NULL REFERENCES accounts (account_id),
    collect_after TIMESTAMP NOT NULL,
    pending BOOLEAN NOT NULL,
    CONSTRAINT component_uploads_pk PRIMARY KEY (object_key)
);

CREATE TABLE account_component_digests (
    account_id UUID NOT NULL REFERENCES accounts (account_id),
    digest TEXT NOT NULL,
    object_key TEXT NOT NULL REFERENCES component_uploads (object_key),
    CONSTRAINT account_component_digests_pk PRIMARY KEY (account_id, digest)
);

CREATE UNIQUE INDEX account_component_digests_object_key_uk ON account_component_digests (object_key);
CREATE INDEX component_revisions_object_store_key_idx ON component_revisions (object_store_key);
