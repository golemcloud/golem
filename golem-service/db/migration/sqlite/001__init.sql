CREATE TABLE templates
(
    template_id        uuid    NOT NULL,
    name               text    NOT NULL,
    size               integer NOT NULL,
    version            integer NOT NULL,
    created_at         timestamp without time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    user_template      text    NOT NULL,
    protected_template text    NOT NULL,
    protector_version  integer,
    metadata           jsonb   NOT NULL,
    PRIMARY KEY (template_id, version)
);
