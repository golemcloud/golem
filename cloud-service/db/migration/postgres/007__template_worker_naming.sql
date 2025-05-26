ALTER TABLE plans RENAME COLUMN component_limit TO template_limit;
ALTER TABLE plans RENAME COLUMN instance_limit TO worker_limit;

ALTER TABLE project_policies RENAME COLUMN view_component TO view_template;
ALTER TABLE project_policies RENAME COLUMN create_component TO create_template;
ALTER TABLE project_policies RENAME COLUMN update_component TO update_template;
ALTER TABLE project_policies RENAME COLUMN delete_component TO delete_template;
ALTER TABLE project_policies RENAME COLUMN view_instance TO view_worker;
ALTER TABLE project_policies RENAME COLUMN create_instance TO create_worker;
ALTER TABLE project_policies RENAME COLUMN update_instance TO update_worker;
ALTER TABLE project_policies RENAME COLUMN delete_instance TO delete_worker;

ALTER TABLE account_instances RENAME TO account_workers;

ALTER TABLE components RENAME TO templates;
ALTER TABLE templates RENAME COLUMN component_id TO template_id;
ALTER TABLE templates RENAME COLUMN user_component TO user_template;
ALTER TABLE templates RENAME COLUMN protected_component TO protected_template;