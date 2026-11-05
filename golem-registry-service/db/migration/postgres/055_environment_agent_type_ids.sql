CREATE TABLE environment_agent_type_ids
(
    environment_id UUID   NOT NULL,
    kind           TEXT   NOT NULL,
    name           TEXT   NOT NULL,
    type_id        BIGINT NOT NULL,

    CONSTRAINT environment_agent_type_ids_pk
        PRIMARY KEY (environment_id, kind, name),
    CONSTRAINT environment_agent_type_ids_environment_fk
        FOREIGN KEY (environment_id) REFERENCES environments,
    CONSTRAINT environment_agent_type_ids_kind_check
        CHECK (kind IN ('agent-type', 'tool')),
    CONSTRAINT environment_agent_type_ids_type_id_check
        CHECK (type_id BETWEEN 1 AND 4294967295)
);

CREATE UNIQUE INDEX environment_agent_type_ids_environment_type_id_uk
    ON environment_agent_type_ids (environment_id, type_id);
