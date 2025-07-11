alter table component_plugin_installation alter column component_id set not null;
alter table component_plugin_installation alter column component_version set not null;

alter table component_plugin_installation drop constraint component_plugin_installation_pkey;
alter table component_plugin_installation add constraint component_plugin_installation_pkey primary key (installation_id, component_id, component_version);
