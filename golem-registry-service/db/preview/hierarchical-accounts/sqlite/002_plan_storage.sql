-- Plan definitions owned by accounts, their immutable revisions and account assignments.
--
-- Preview schema for hierarchical accounts. Tests apply it on top of the production migrations;
-- the production migration flow does not include it.

-- An assignment references its account together with that account's parent, so the parent of
-- the assigned account must own the definition.
CREATE UNIQUE INDEX account_nodes_parent_link
    ON account_nodes (account_id, parent_id);

CREATE TABLE account_plans
(
    plan_id          UUID      NOT NULL,
    owner_account_id UUID      NOT NULL,
    name             TEXT      NOT NULL,
    selectable       BOOLEAN   NOT NULL,
    current_revision BIGINT    NOT NULL,
    created_at       TIMESTAMP NOT NULL,
    updated_at       TIMESTAMP NOT NULL,
    modified_by      UUID      NOT NULL,

    CONSTRAINT account_plans_pk
        PRIMARY KEY (plan_id),
    CONSTRAINT account_plans_owner_fk
        FOREIGN KEY (owner_account_id) REFERENCES account_nodes (account_id),
    CONSTRAINT account_plans_owner_plan
        UNIQUE (owner_account_id, plan_id),
    CONSTRAINT account_plans_revision
        CHECK (current_revision >= 0)
);

-- Names are unique among an owner's selectable definitions.
CREATE UNIQUE INDEX account_plans_owner_name
    ON account_plans (owner_account_id, name)
    WHERE selectable = 1;

-- Revisions are history and have no foreign key, so they outlive their definition.
CREATE TABLE account_plan_revisions
(
    plan_id          UUID      NOT NULL,
    revision         BIGINT    NOT NULL,
    owner_account_id UUID      NOT NULL,
    name             TEXT      NOT NULL,
    created_at       TIMESTAMP NOT NULL,
    created_by       UUID      NOT NULL,

    CONSTRAINT account_plan_revisions_pk
        PRIMARY KEY (plan_id, revision)
);

-- The typed declaration of one dimension in one revision. A dimension without a row is not
-- declared by the revision.
CREATE TABLE account_plan_revision_bounds
(
    plan_id           UUID     NOT NULL,
    revision          BIGINT   NOT NULL,
    dimension         SMALLINT NOT NULL,
    min_value         NUMERIC,
    max_value         NUMERIC,
    max_unlimited     BOOLEAN  NOT NULL,
    default_value     NUMERIC,
    default_unlimited BOOLEAN  NOT NULL,
    user_configurable BOOLEAN,

    CONSTRAINT account_plan_revision_bounds_pk
        PRIMARY KEY (plan_id, revision, dimension),
    CONSTRAINT account_plan_revision_bounds_revision_fk
        FOREIGN KEY (plan_id, revision) REFERENCES account_plan_revisions (plan_id, revision),
    CONSTRAINT account_plan_revision_bounds_max
        CHECK (NOT (max_unlimited AND max_value IS NOT NULL)),
    CONSTRAINT account_plan_revision_bounds_default
        CHECK (NOT (default_unlimited AND default_value IS NOT NULL))
);

CREATE TABLE account_plan_assignments
(
    account_id  UUID      NOT NULL,
    parent_id   UUID      NOT NULL,
    plan_id     UUID      NOT NULL,
    revision    BIGINT    NOT NULL,
    updated_at  TIMESTAMP NOT NULL,
    modified_by UUID      NOT NULL,

    CONSTRAINT account_plan_assignments_pk
        PRIMARY KEY (account_id),
    CONSTRAINT account_plan_assignments_account_fk
        FOREIGN KEY (account_id, parent_id) REFERENCES account_nodes (account_id, parent_id),
    CONSTRAINT account_plan_assignments_plan_fk
        FOREIGN KEY (parent_id, plan_id) REFERENCES account_plans (owner_account_id, plan_id),
    CONSTRAINT account_plan_assignments_revision
        CHECK (revision >= 0)
);

CREATE INDEX account_plan_assignments_plan
    ON account_plan_assignments (plan_id);

-- Each assignment revision names the definition revision that was current when it committed.
CREATE TABLE account_plan_assignment_revisions
(
    account_id    UUID      NOT NULL,
    revision      BIGINT    NOT NULL,
    plan_id       UUID      NOT NULL,
    plan_revision BIGINT    NOT NULL,
    created_at    TIMESTAMP NOT NULL,
    created_by    UUID      NOT NULL,

    CONSTRAINT account_plan_assignment_revisions_pk
        PRIMARY KEY (account_id, revision)
);
