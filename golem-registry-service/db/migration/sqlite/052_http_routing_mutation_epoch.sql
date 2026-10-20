ALTER TABLE environments
    ADD COLUMN http_routing_mutation_epoch BIGINT NOT NULL DEFAULT 0;
