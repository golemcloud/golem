CREATE TABLE api_definitions
(
    namespace  text      NOT NULL default '',
    id         uuid      NOT NULL,
    version    text      NOT NULL,
    draft      boolean   NOT NULL default true,
    routes     jsonb     NOT NULL,
    created_at timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (namespace, id, version)
);


CREATE TABLE api_deployments
(
    namespace          text      NOT NULL default '',
    host               text      NOT NULL,
    subdomain          text,
    definition_id      uuid      NOT NULL,
    definition_version text      NOT NULL,
    created_at         timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (namespace, host, subdomain, definition_id, definition_version)
);

