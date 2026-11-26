CREATE TABLE packages (
    package_path TEXT NOT NULL,
    publisher_account_id UUID NOT NULL REFERENCES accounts (account_id),
    CONSTRAINT packages_pk PRIMARY KEY (package_path)
);

CREATE TABLE package_versions (
    package_path TEXT NOT NULL REFERENCES packages (package_path),
    version TEXT NOT NULL,
    manifest_digest TEXT NOT NULL,
    archive_digest TEXT NOT NULL,
    lifecycle SMALLINT NOT NULL DEFAULT 0,
    description TEXT,
    callables JSONB NOT NULL,
    CONSTRAINT package_versions_pk PRIMARY KEY (package_path, version)
);
