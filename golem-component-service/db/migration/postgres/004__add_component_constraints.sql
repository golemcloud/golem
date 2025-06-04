CREATE TABLE component_constraints
(
    component_id        uuid    NOT NULL,
    namespace           text    NOT NULL,
    constraints         bytea   NOT NULL,
    PRIMARY KEY (component_id, namespace)
);
