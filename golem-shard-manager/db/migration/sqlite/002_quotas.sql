CREATE TABLE quota_resources
(
    resource_definition_id UUID NOT NULL,
    revision               BIGINT NOT NULL,
    definition             BYTEA NOT NULL,
    remaining              NUMERIC NOT NULL,
    last_refilled_at       TIMESTAMP NOT NULL,
    last_refreshed_at      TIMESTAMP NOT NULL,

    CONSTRAINT quota_resources_pk
        PRIMARY KEY (resource_definition_id)
);

CREATE TABLE quota_leases
(
    resource_definition_id UUID NOT NULL,
    pod_ip                 TEXT NOT NULL,
    pod_port               INTEGER NOT NULL,
    epoch                  NUMERIC NOT NULL,
    allocated              NUMERIC NOT NULL,
    pending_reservations   BYTEA NOT NULL,
    granted_at             TIMESTAMP NOT NULL,
    expires_at             TIMESTAMP NOT NULL,

    CONSTRAINT quota_leases_pk
        PRIMARY KEY (resource_definition_id, pod_ip, pod_port),
    CONSTRAINT quota_leases_resource_fk
        FOREIGN KEY (resource_definition_id) REFERENCES quota_resources
);
