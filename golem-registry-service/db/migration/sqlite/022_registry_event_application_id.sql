ALTER TABLE registry_change_events
    ADD application_id TEXT;

-- environment_ids: used by ApplicationDeleted (stored as JSON array of UUID strings).
ALTER TABLE registry_change_events
    ADD environment_ids TEXT;

-- app_name: used by ApplicationDeleted and EnvironmentDeleted.
ALTER TABLE registry_change_events
    ADD app_name TEXT;

-- env_name: used by EnvironmentDeleted.
ALTER TABLE registry_change_events
    ADD env_name TEXT;
