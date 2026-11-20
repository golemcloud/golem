-- Account tree: current projection and immutable history.
--
-- Test-only schema for hierarchical accounts. Only the repository tests install it, on top of the
-- production migrations. The production migrations do not include it, and no live repository or
-- service reads it. HA-15 moves it into the production migrations and the active account path.
--
-- The repository never changes a node's parent or depth. The database does not enforce that rule;
-- it enforces the parent foreign key, the sibling key, one root and the CHECK constraints below.

CREATE TABLE account_nodes
(
    account_id       UUID      NOT NULL,
    parent_id        UUID,
    segment          TEXT      NOT NULL,
    depth            SMALLINT  NOT NULL,
    display_name     TEXT,
    own_state        SMALLINT  NOT NULL,
    current_revision BIGINT    NOT NULL,
    created_at       TIMESTAMP NOT NULL,
    updated_at       TIMESTAMP NOT NULL,
    modified_by      UUID      NOT NULL,

    CONSTRAINT account_nodes_pk
        PRIMARY KEY (account_id),
    CONSTRAINT account_nodes_parent_fk
        FOREIGN KEY (parent_id) REFERENCES account_nodes (account_id),
    CONSTRAINT account_nodes_sibling_key
        UNIQUE (parent_id, segment),
    CONSTRAINT account_nodes_root_shape
        CHECK ((parent_id IS NULL AND segment = '' AND depth = 0)
            OR (parent_id IS NOT NULL AND segment <> '' AND depth BETWEEN 1 AND 8)),
    CONSTRAINT account_nodes_segment_length
        CHECK (char_length(segment) <= 64),
    CONSTRAINT account_nodes_own_state
        CHECK (own_state BETWEEN 0 AND 2),
    CONSTRAINT account_nodes_revision
        CHECK (current_revision >= 0)
);

-- Every root has the empty segment, so this admits at most one root.
CREATE UNIQUE INDEX account_nodes_single_root
    ON account_nodes (segment)
    WHERE parent_id IS NULL;

-- History has no foreign key to the current row, so it outlives a purge.
CREATE TABLE account_node_revisions
(
    account_id   UUID      NOT NULL,
    revision     BIGINT    NOT NULL,
    parent_id    UUID,
    segment      TEXT      NOT NULL,
    display_name TEXT,
    own_state    SMALLINT  NOT NULL,
    created_at   TIMESTAMP NOT NULL,
    created_by   UUID      NOT NULL,

    CONSTRAINT account_node_revisions_pk
        PRIMARY KEY (account_id, revision)
);
