CREATE TABLE components
(
    component_id        uuid    NOT NULL,
    project_id          uuid    NOT NULL,
    name                text    NOT NULL,
    size                integer NOT NULL,
    version             bigint  NOT NULL,
    created_at          timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    user_component      text    NOT NULL,
    protected_component text    NOT NULL,
    protector_version   bigint,
    metadata            jsonb   NOT NULL,
    PRIMARY KEY (component_id, version)
);