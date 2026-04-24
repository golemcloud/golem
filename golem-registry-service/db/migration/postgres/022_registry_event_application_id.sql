ALTER TABLE registry_change_events
    ADD application_id UUID;

-- environment_ids: used by ApplicationDeleted to carry the UUIDs of all
-- non-deleted environments under the deleted application.
ALTER TABLE registry_change_events
    ADD environment_ids UUID[];

-- app_name: used by ApplicationDeleted and EnvironmentDeleted to carry the
-- application's human-readable name for targeted cache invalidation.
ALTER TABLE registry_change_events
    ADD app_name TEXT;

-- env_name: used by EnvironmentDeleted to carry the environment's
-- human-readable name for targeted cache invalidation.
ALTER TABLE registry_change_events
    ADD env_name TEXT;
