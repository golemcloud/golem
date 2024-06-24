CREATE TABLE api_definitions
(
    namespace  text    NOT NULL default '',
    id         text    NOT NULL,
    version    text    NOT NULL,
    draft      boolean NOT NULL default true,
    data       jsonb   NOT NULL,
    created_at timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    PRIMARY KEY (namespace, id, version)
);


CREATE TABLE api_deployments
(
    namespace          text NOT NULL default '',
    host               text NOT NULL,
    subdomain          text,
    definition_id      text NOT NULL,
    definition_version text NOT NULL,
    created_at         timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    PRIMARY KEY (namespace, host, subdomain, definition_id, definition_version)
);
