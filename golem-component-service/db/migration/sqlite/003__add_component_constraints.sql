CREATE TABLE component_constraints
(
    component_id        uuid    NOT NULL,
    namespace           text    NOT NULL,
    constraints         blob    NOT NULL,
    PRIMARY KEY (component_id, namespace)
);
