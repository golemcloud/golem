// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use golem_common::model::card::owner::{
    AgentOwnerLeafPattern, AgentOwnerPattern, EmptyOwnerPattern, EnvironmentOwnerPattern,
    ToolOwnerPattern,
};
use golem_common::model::card::*;
use sqlparser::ast::{
    AlterTableOperation, ColumnDef, ColumnOption, CreateTableLikeKind, FromTable, ObjectName,
    ObjectNamePart, ObjectType, Query, RenameTableNameKind, Select, SetExpr, Statement,
    TableConstraint, TableFactor, TableObject, Visit, Visitor, visit_relations,
};
use sqlparser::dialect::{GenericDialect, MySqlDialect, PostgreSqlDialect};
use sqlparser::parser::Parser;
use std::fmt::{Display, Formatter};
use std::net::IpAddr;
use std::ops::ControlFlow;
use url::Url;

use crate::durable_host::DurableWorkerCtx;
use crate::workerctx::WorkerCtx;

pub fn agent_owner<Ctx: WorkerCtx>(ctx: &DurableWorkerCtx<Ctx>) -> AgentOwnerPattern {
    let component = ctx.owner_component_metadata();
    AgentOwnerPattern::Agent {
        account: component.account_email.clone(),
        application: component.application_name.clone(),
        environment: component.environment_name.clone(),
        component: component.component_name.clone(),
        agent: AgentOwnerLeafPattern::Agent(ctx.agent_id().agent_id.clone()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetError {
    InvalidPath(String),
    InvalidResource { class: &'static str, value: String },
    InvalidNetworkAddress(String),
    InvalidUrl(String),
    SqlNotStaticallyExtractable(String),
}

impl Display for TargetError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPath(v) => write!(f, "invalid guest path: {v}"),
            Self::InvalidResource { class, value } => {
                write!(f, "invalid {class} resource: {value}")
            }
            Self::InvalidNetworkAddress(v) => write!(f, "invalid network address: {v}"),
            Self::InvalidUrl(v) => write!(f, "invalid outbound URL: {v}"),
            Self::SqlNotStaticallyExtractable(v) => {
                write!(f, "SQL resource set cannot be extracted safely: {v}")
            }
        }
    }
}

impl std::error::Error for TargetError {}

fn resource<R: ResourcePattern>(class: &'static str, value: &str) -> Result<R, TargetError> {
    R::parse_resource(value).map_err(|_| TargetError::InvalidResource {
        class,
        value: value.to_string(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CanonicalGuestPath(String);

impl CanonicalGuestPath {
    pub fn new(path: &str) -> Result<Self, TargetError> {
        if !path.starts_with('/') || path.contains('\0') {
            return Err(TargetError::InvalidPath(path.to_string()));
        }
        let mut segments = Vec::new();
        for segment in path.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    if segments.pop().is_none() {
                        return Err(TargetError::InvalidPath(path.to_string()));
                    }
                }
                value if value.contains('*') => {
                    return Err(TargetError::InvalidPath(path.to_string()));
                }
                value => segments.push(value),
            }
        }
        Ok(Self(if segments.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", segments.join("/"))
        }))
    }

    pub fn resolve(&self, relative: &str) -> Result<Self, TargetError> {
        if relative.starts_with('/') {
            return Err(TargetError::InvalidPath(relative.to_string()));
        }
        let mut segments = self
            .0
            .split('/')
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>();
        let descriptor_depth = segments.len();
        for segment in relative.split('/') {
            match segment {
                "" | "." => {}
                ".." if segments.len() > descriptor_depth => {
                    segments.pop();
                }
                ".." => return Err(TargetError::InvalidPath(relative.to_string())),
                value if value.contains(['*', '\0']) => {
                    return Err(TargetError::InvalidPath(relative.to_string()));
                }
                value => segments.push(value),
            }
        }
        Self::new(&format!("/{}", segments.join("/")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn filesystem_target(
    owner: AgentOwnerPattern,
    verb: FilesystemVerb,
    path: &CanonicalGuestPath,
) -> PermissionTarget {
    PermissionTarget::Filesystem(ClassPermissionTarget {
        verb: Some(verb),
        owner,
        resource: resource("filesystem", path.as_str())
            .expect("canonical paths satisfy filesystem grammar"),
    })
}

pub fn network_target(host: &str, port: Option<u16>) -> Result<PermissionTarget, TargetError> {
    let host = normalize_host(host)?;
    Ok(PermissionTarget::Network(ClassPermissionTarget {
        verb: Some(NetworkVerb::Connect),
        owner: EmptyOwnerPattern,
        resource: NetworkResourcePattern::host_port(
            host,
            port.map(PortPattern::single)
                .unwrap_or_else(PortPattern::any),
        ),
    }))
}

pub fn dns_target(host: &str) -> Result<PermissionTarget, TargetError> {
    network_target(host, None)
}

pub fn tcp_target(host: &str, port: u16) -> Result<PermissionTarget, TargetError> {
    network_target(host, Some(port))
}

pub fn udp_target(host: &str, port: u16) -> Result<PermissionTarget, TargetError> {
    network_target(host, Some(port))
}

fn normalize_host(host: &str) -> Result<String, TargetError> {
    let host = host.trim().trim_end_matches('.');
    if host.is_empty()
        || host.contains(':')
        || host.contains('*')
        || host.chars().any(char::is_whitespace)
    {
        return Err(TargetError::InvalidNetworkAddress(host.to_string()));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v4) => Ok(v4.to_string()),
            IpAddr::V6(_) => Err(TargetError::InvalidNetworkAddress(host.to_string())),
        };
    }
    if host.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err(TargetError::InvalidNetworkAddress(host.to_string()));
    }
    let normalized = host.to_ascii_lowercase();
    resource::<NetworkResourcePattern>("network", &normalized)?;
    Ok(normalized)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedHttpTarget {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub path: String,
    pub permission: PermissionTarget,
}

pub fn http_target(uri: &str) -> Result<NormalizedHttpTarget, TargetError> {
    uri_target(uri, &["http", "https"])
}

pub fn websocket_target(uri: &str) -> Result<NormalizedHttpTarget, TargetError> {
    uri_target(uri, &["ws", "wss"])
}

fn uri_target(uri: &str, schemes: &[&str]) -> Result<NormalizedHttpTarget, TargetError> {
    let url = Url::parse(uri).map_err(|_| TargetError::InvalidUrl(uri.to_string()))?;
    if !schemes.contains(&url.scheme()) || !url.username().is_empty() || url.password().is_some() {
        return Err(TargetError::InvalidUrl(uri.to_string()));
    }
    let host = normalize_host(
        url.host_str()
            .ok_or_else(|| TargetError::InvalidUrl(uri.to_string()))?,
    )?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| TargetError::InvalidUrl(uri.to_string()))?;
    Ok(NormalizedHttpTarget {
        scheme: url.scheme().to_ascii_lowercase(),
        host: host.clone(),
        port,
        path: url.path().to_string(),
        permission: network_target(&host, Some(port))?,
    })
}

pub fn env_target(owner: AgentOwnerPattern, name: &str) -> Result<PermissionTarget, TargetError> {
    Ok(PermissionTarget::Env(ClassPermissionTarget {
        verb: Some(EnvVerb::Read),
        owner,
        resource: resource("env", name)?,
    }))
}

pub fn kv_target(
    owner: EnvironmentOwnerPattern,
    verb: KvVerb,
    bucket: &str,
    key: &str,
) -> Result<PermissionTarget, TargetError> {
    Ok(PermissionTarget::Kv(ClassPermissionTarget {
        verb: Some(verb),
        owner,
        resource: KvResourcePattern::StoreKey {
            store: bucket.to_string(),
            key_pattern: key.to_string(),
        },
    }))
}

pub fn kv_bucket_target(
    owner: EnvironmentOwnerPattern,
    verb: KvVerb,
    bucket: &str,
) -> Result<PermissionTarget, TargetError> {
    kv_target(owner, verb, bucket, "**")
}

pub fn blob_target(
    owner: EnvironmentOwnerPattern,
    verb: BlobVerb,
    container: &str,
    object: &str,
) -> Result<PermissionTarget, TargetError> {
    Ok(PermissionTarget::Blob(ClassPermissionTarget {
        verb: Some(verb),
        owner,
        resource: BlobResourcePattern::BucketKey {
            bucket: container.to_string(),
            key_pattern: object.to_string(),
        },
    }))
}

pub fn blob_container_target(
    owner: EnvironmentOwnerPattern,
    verb: BlobVerb,
    container: &str,
) -> Result<PermissionTarget, TargetError> {
    blob_target(owner, verb, container, "**")
}

pub fn secret_target(
    owner: EnvironmentOwnerPattern,
    verb: SecretVerb,
    key: &str,
) -> Result<PermissionTarget, TargetError> {
    Ok(PermissionTarget::Secret(ClassPermissionTarget {
        verb: Some(verb),
        owner,
        resource: resource("secret", key)?,
    }))
}

pub fn config_segments_target(
    owner: AgentOwnerPattern,
    segments: &[String],
) -> Result<PermissionTarget, TargetError> {
    if segments.is_empty()
        || segments
            .iter()
            .any(|segment| segment.is_empty() || segment.contains(['.', '*']))
    {
        return Err(TargetError::InvalidResource {
            class: "config",
            value: segments.join("."),
        });
    }
    Ok(PermissionTarget::Config(ClassPermissionTarget {
        verb: Some(ConfigVerb::Read),
        owner,
        resource: ConfigResourcePattern::Key(ConfigKeyPathPattern {
            segments: segments
                .iter()
                .cloned()
                .map(ConfigKeySegmentPattern::Literal)
                .collect(),
        }),
    }))
}

pub fn oplog_target(
    owner: AgentOwnerPattern,
    start: Option<u64>,
    end: Option<u64>,
) -> Result<PermissionTarget, TargetError> {
    if start.zip(end).is_some_and(|(s, e)| s > e) {
        return Err(TargetError::InvalidResource {
            class: "oplog",
            value: format!("{start:?}..{end:?}"),
        });
    }
    Ok(PermissionTarget::Oplog(ClassPermissionTarget {
        verb: Some(OplogVerb::Read),
        owner,
        resource: OplogResourcePattern::range(start, end),
    }))
}

pub fn agent_target(
    owner: AgentOwnerPattern,
    verb: AgentVerb,
    resource: AgentResourcePattern,
) -> PermissionTarget {
    PermissionTarget::Agent(ClassPermissionTarget {
        verb: Some(verb),
        owner,
        resource,
    })
}

pub fn agent_method_target(
    owner: AgentOwnerPattern,
    verb: AgentVerb,
    method: &str,
) -> Result<PermissionTarget, TargetError> {
    Ok(agent_target(owner, verb, resource("agent", method)?))
}

pub fn agent_worker_target(
    owner: AgentOwnerPattern,
    verb: AgentVerb,
    resource: AgentResourcePattern,
) -> PermissionTarget {
    agent_target(owner, verb, resource)
}

pub fn tool_target(
    owner: ToolOwnerPattern,
    command_path: &[&str],
    args: &[&str],
) -> Result<PermissionTarget, TargetError> {
    let invocation =
        ToolInvocationPattern::from_command_and_args(command_path, args).map_err(|value| {
            TargetError::InvalidResource {
                class: "tool",
                value,
            }
        })?;
    Ok(PermissionTarget::Tool(ClassPermissionTarget {
        verb: Some(ToolVerb::Invoke),
        owner,
        resource: ToolResourcePattern::Invocation(invocation),
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RdbmsEngine {
    Postgres,
    Mysql,
    Ignite,
}

pub fn rdbms_target(
    owner: EnvironmentOwnerPattern,
    verb: RdbmsVerb,
    database: &str,
    schema: &str,
    table: &str,
) -> Result<PermissionTarget, TargetError> {
    let value = format!("{database}.{schema}.{table}");
    Ok(PermissionTarget::Rdbms(ClassPermissionTarget {
        verb: Some(verb),
        owner,
        resource: resource("rdbms", &value)?,
    }))
}

pub fn rdbms_sql_targets(
    owner: EnvironmentOwnerPattern,
    engine: RdbmsEngine,
    database: &str,
    default_schema: &str,
    sql: &str,
) -> Result<Vec<PermissionTarget>, TargetError> {
    let statements = match engine {
        RdbmsEngine::Postgres => Parser::parse_sql(&PostgreSqlDialect {}, sql),
        RdbmsEngine::Mysql => Parser::parse_sql(&MySqlDialect {}, sql),
        RdbmsEngine::Ignite => Parser::parse_sql(&GenericDialect {}, sql),
    };
    let mut statements = match statements {
        Ok(statements) if statements.len() == 1 => statements,
        Ok(_) => return sql_fallback("exactly one statement is required"),
        Err(error) => return sql_fallback(&error.to_string()),
    };
    let statement = statements.pop().expect("one parsed statement");
    let names = match extract_sql_relations(&statement) {
        Ok(names) => names,
        Err(reason) => return sql_fallback(&reason),
    };

    let targets = names
        .into_iter()
        .map(|(name, verb)| {
            let parts = object_name_parts(&name)?;
            let (db, schema, table) = match parts.as_slice() {
                [table] => (database, default_schema, table.as_str()),
                [qualifier, table] if engine == RdbmsEngine::Mysql => {
                    (qualifier.as_str(), qualifier.as_str(), table.as_str())
                }
                [schema, table] => (database, schema.as_str(), table.as_str()),
                [db, schema, table] => (db.as_str(), schema.as_str(), table.as_str()),
                _ => unreachable!(),
            };
            Ok(PermissionTarget::Rdbms(ClassPermissionTarget {
                verb: Some(verb),
                owner: owner.clone(),
                resource: RdbmsResourcePattern::Table {
                    database: db.to_string(),
                    schema: schema.to_string(),
                    table: table.to_string(),
                },
            }))
        })
        .collect::<Result<Vec<_>, TargetError>>();
    match targets {
        Ok(targets) => Ok(targets),
        Err(TargetError::SqlNotStaticallyExtractable(reason)) => sql_fallback(&reason),
        Err(error) => Err(error),
    }
}

fn extract_sql_relations(statement: &Statement) -> Result<Vec<(ObjectName, RdbmsVerb)>, String> {
    validate_sql_ast(statement)?;

    let mut mutations = Vec::new();
    let mut queries = Vec::new();
    match statement {
        Statement::Query(_) => {}
        Statement::Insert(insert) => {
            let TableObject::TableName(name) = &insert.table else {
                return Err("insert destination is not a concrete table".to_string());
            };
            push_unique(&mut mutations, name.clone());
            if let Some(source) = &insert.source {
                collect_relations(source.as_ref(), &mut queries);
            }
            if let Some(returning) = &insert.returning {
                collect_relations(returning, &mut queries);
            }
        }
        Statement::Update(update) => {
            plain_table_factor_name(&update.table.relation)?;
            collect_relations(&update.table, &mut mutations);
            if mutations.is_empty() {
                return Err("update destination is not a concrete table".to_string());
            }
            if let Some(from) = &update.from {
                collect_relations(from, &mut queries);
            }
            collect_relations(&update.assignments, &mut queries);
            if let Some(selection) = &update.selection {
                collect_relations(selection, &mut queries);
            }
            if let Some(returning) = &update.returning {
                collect_relations(returning, &mut queries);
            }
        }
        Statement::Delete(delete) => {
            if !delete.tables.is_empty() {
                return Err("multi-table delete targets are ambiguous".to_string());
            }
            let from = match &delete.from {
                FromTable::WithFromKeyword(from) | FromTable::WithoutKeyword(from) => from,
            };
            let [table] = from.as_slice() else {
                return Err("delete requires one concrete destination table".to_string());
            };
            if !table.joins.is_empty() {
                return Err("delete destination joins are ambiguous".to_string());
            }
            push_unique(
                &mut mutations,
                plain_table_factor_name(&table.relation)?.clone(),
            );
            if let Some(using) = &delete.using {
                collect_relations(using, &mut queries);
            }
            if let Some(selection) = &delete.selection {
                collect_relations(selection, &mut queries);
            }
            if let Some(returning) = &delete.returning {
                collect_relations(returning, &mut queries);
            }
            collect_relations(&delete.order_by, &mut queries);
            if let Some(limit) = &delete.limit {
                collect_relations(limit, &mut queries);
            }
        }
        Statement::Merge(merge) => {
            push_unique(
                &mut mutations,
                plain_table_factor_name(&merge.table)?.clone(),
            );
            collect_relations(&merge.source, &mut queries);
            collect_relations(&merge.on, &mut queries);
            collect_relations(&merge.clauses, &mut queries);
        }
        Statement::Truncate(truncate) => {
            for table in &truncate.table_names {
                push_unique(&mut mutations, table.name.clone());
            }
        }
        Statement::CreateTable(create) => {
            push_unique(&mut mutations, create.name.clone());
            if let Some(query) = &create.query {
                collect_relations(query.as_ref(), &mut queries);
            }
            if let Some(like) = &create.like {
                let name = match like {
                    CreateTableLikeKind::Parenthesized(like) | CreateTableLikeKind::Plain(like) => {
                        &like.name
                    }
                };
                push_unique(&mut queries, name.clone());
            }
            if let Some(clone) = &create.clone {
                push_unique(&mut queries, clone.clone());
            }
            if let Some(inherits) = &create.inherits {
                for name in inherits {
                    push_unique(&mut queries, name.clone());
                }
            }
            if let Some(partition_of) = &create.partition_of {
                push_unique(&mut queries, partition_of.clone());
            }
            collect_column_foreign_tables(&create.columns, &mut queries);
            collect_constraint_foreign_tables(&create.constraints, &mut queries);
        }
        Statement::AlterTable(alter) => {
            push_unique(&mut mutations, alter.name.clone());
            for operation in &alter.operations {
                match operation {
                    AlterTableOperation::AddConstraint { constraint, .. } => {
                        collect_constraint_foreign_tables(
                            std::slice::from_ref(constraint),
                            &mut queries,
                        );
                    }
                    AlterTableOperation::AddColumn { column_def, .. } => {
                        collect_column_foreign_tables(
                            std::slice::from_ref(column_def),
                            &mut queries,
                        );
                    }
                    AlterTableOperation::RenameTable { table_name } => {
                        let name = match table_name {
                            RenameTableNameKind::As(name) | RenameTableNameKind::To(name) => name,
                        };
                        push_unique(&mut mutations, name.clone());
                    }
                    AlterTableOperation::SwapWith { table_name } => {
                        push_unique(&mut mutations, table_name.clone());
                    }
                    AlterTableOperation::ChangeColumn { options, .. }
                    | AlterTableOperation::ModifyColumn { options, .. } => {
                        collect_column_options_foreign_tables(options, &mut queries);
                    }
                    AlterTableOperation::DropConstraint { .. }
                    | AlterTableOperation::DropColumn { .. }
                    | AlterTableOperation::DropPrimaryKey { .. }
                    | AlterTableOperation::DropForeignKey { .. }
                    | AlterTableOperation::DropIndex { .. }
                    | AlterTableOperation::RenameColumn { .. }
                    | AlterTableOperation::RenameConstraint { .. }
                    | AlterTableOperation::AlterColumn { .. } => {}
                    _ => return Err("unsupported alter-table operation".to_string()),
                }
            }
        }
        Statement::Drop {
            object_type,
            names,
            table,
            ..
        } if *object_type == ObjectType::Table && table.is_none() => {
            for name in names {
                push_unique(&mut mutations, name.clone());
            }
        }
        _ => return Err("unsupported statement".to_string()),
    }

    let mut all_relations = Vec::new();
    collect_relations(statement, &mut all_relations);
    for relation in all_relations {
        if !mutations.contains(&relation) {
            push_unique(&mut queries, relation);
        }
    }

    let mut result = Vec::new();
    for name in mutations {
        push_unique_relation(&mut result, name, RdbmsVerb::Mutate);
    }
    for name in queries {
        push_unique_relation(&mut result, name, RdbmsVerb::Query);
    }
    Ok(result)
}

fn validate_sql_ast(statement: &Statement) -> Result<(), String> {
    let mut validator = SqlAstValidator;
    match statement.visit(&mut validator) {
        ControlFlow::Continue(()) => Ok(()),
        ControlFlow::Break(reason) => Err(reason.to_string()),
    }
}

struct SqlAstValidator;

impl Visitor for SqlAstValidator {
    type Break = &'static str;

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<Self::Break> {
        if query.with.is_some()
            || !query.locks.is_empty()
            || query.for_clause.is_some()
            || query.settings.is_some()
            || query.format_clause.is_some()
            || !query.pipe_operators.is_empty()
            || !read_only_set_expr(&query.body)
        {
            ControlFlow::Break("unsupported or effectful query form")
        } else {
            ControlFlow::Continue(())
        }
    }

    fn pre_visit_select(&mut self, select: &Select) -> ControlFlow<Self::Break> {
        if select.into.is_some() || !select.lateral_views.is_empty() {
            ControlFlow::Break("unsupported or effectful select form")
        } else {
            ControlFlow::Continue(())
        }
    }

    fn pre_visit_table_factor(&mut self, factor: &TableFactor) -> ControlFlow<Self::Break> {
        match factor {
            TableFactor::Table { args: None, .. } | TableFactor::NestedJoin { .. } => {
                ControlFlow::Continue(())
            }
            TableFactor::Derived { subquery, .. }
                if !matches!(subquery.body.as_ref(), SetExpr::Values(_)) =>
            {
                ControlFlow::Continue(())
            }
            _ => ControlFlow::Break("unsupported table expression"),
        }
    }
}

fn read_only_set_expr(expr: &SetExpr) -> bool {
    match expr {
        SetExpr::Select(_) | SetExpr::Values(_) => true,
        SetExpr::Query(query) => read_only_set_expr(&query.body),
        SetExpr::SetOperation { left, right, .. } => {
            read_only_set_expr(left) && read_only_set_expr(right)
        }
        SetExpr::Insert(_)
        | SetExpr::Update(_)
        | SetExpr::Delete(_)
        | SetExpr::Merge(_)
        | SetExpr::Table(_) => false,
    }
}

fn plain_table_factor_name(factor: &TableFactor) -> Result<&ObjectName, String> {
    match factor {
        TableFactor::Table {
            name, args: None, ..
        } => Ok(name),
        _ => Err("destination is not a concrete table".to_string()),
    }
}

fn collect_relations<T: Visit>(value: &T, result: &mut Vec<ObjectName>) {
    let _: ControlFlow<()> = visit_relations(value, |name| {
        push_unique(result, name.clone());
        ControlFlow::Continue(())
    });
}

fn collect_column_foreign_tables(columns: &[ColumnDef], result: &mut Vec<ObjectName>) {
    for option in columns.iter().flat_map(|column| &column.options) {
        collect_column_option_foreign_table(&option.option, result);
    }
}

fn collect_column_options_foreign_tables(options: &[ColumnOption], result: &mut Vec<ObjectName>) {
    for option in options {
        collect_column_option_foreign_table(option, result);
    }
}

fn collect_column_option_foreign_table(option: &ColumnOption, result: &mut Vec<ObjectName>) {
    if let ColumnOption::ForeignKey(foreign_key) = option {
        push_unique(result, foreign_key.foreign_table.clone());
    }
}

fn collect_constraint_foreign_tables(
    constraints: &[TableConstraint],
    result: &mut Vec<ObjectName>,
) {
    for constraint in constraints {
        if let TableConstraint::ForeignKey(foreign_key) = constraint {
            push_unique(result, foreign_key.foreign_table.clone());
        }
    }
}

fn push_unique(values: &mut Vec<ObjectName>, value: ObjectName) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn push_unique_relation(
    values: &mut Vec<(ObjectName, RdbmsVerb)>,
    name: ObjectName,
    verb: RdbmsVerb,
) {
    if !values.iter().any(|value| value == &(name.clone(), verb)) {
        values.push((name, verb));
    }
}

fn object_name_parts(name: &ObjectName) -> Result<Vec<String>, TargetError> {
    let parts = name
        .0
        .iter()
        .map(|part| match part {
            ObjectNamePart::Identifier(identifier)
                if !identifier.value.is_empty() && identifier.value != "*" =>
            {
                Ok(identifier.value.clone())
            }
            _ => Err(TargetError::SqlNotStaticallyExtractable(
                "dynamic or invalid table name".to_string(),
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if (1..=3).contains(&parts.len()) {
        Ok(parts)
    } else {
        Err(TargetError::SqlNotStaticallyExtractable(
            "table names must contain one to three parts".to_string(),
        ))
    }
}

fn sql_fallback(reason: &str) -> Result<Vec<PermissionTarget>, TargetError> {
    Err(TargetError::SqlNotStaticallyExtractable(reason.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn agent(name: &str) -> AgentOwnerPattern {
        AgentOwnerPattern::parse(&format!("a@b.com/app/env/component/{name}")).unwrap()
    }
    fn env(name: &str) -> EnvironmentOwnerPattern {
        EnvironmentOwnerPattern::parse(&format!("a@b.com/app/{name}")).unwrap()
    }

    fn tool(name: &str) -> ToolOwnerPattern {
        ToolOwnerPattern::parse(&format!("a@b.com/app/env/component/{name}")).unwrap()
    }

    #[test]
    fn owner_identity_is_part_of_target() {
        let a = filesystem_target(
            agent("one"),
            FilesystemVerb::Read,
            &CanonicalGuestPath::new("/x").unwrap(),
        );
        let b = filesystem_target(
            agent("two"),
            FilesystemVerb::Read,
            &CanonicalGuestPath::new("/x").unwrap(),
        );
        assert_ne!(a, b);
    }

    #[test]
    fn outbound_agent_target_keeps_exact_owner_method_and_verb() {
        let owner = agent("Counter(main)");
        let target = agent_method_target(owner.clone(), AgentVerb::Invoke, "increment").unwrap();
        assert!(matches!(
            target,
            PermissionTarget::Agent(ClassPermissionTarget {
                verb: Some(AgentVerb::Invoke),
                owner: actual_owner,
                resource: AgentResourcePattern::Method(AgentMethodName(method)),
            }) if actual_owner == owner && method == "increment"
        ));
    }

    #[test]
    fn agent_operation_resources_are_operation_specific() {
        let owner = agent("target");
        let invocation = AgentInvocationIdPattern::Uuid(uuid::Uuid::nil());
        let cases = [
            agent_target(owner.clone(), AgentVerb::View, AgentResourcePattern::Empty),
            agent_target(
                owner.clone(),
                AgentVerb::Delete,
                AgentResourcePattern::Empty,
            ),
            agent_target(
                owner.clone(),
                AgentVerb::Interrupt,
                AgentResourcePattern::Empty,
            ),
            agent_target(
                owner.clone(),
                AgentVerb::Resume,
                AgentResourcePattern::Empty,
            ),
            agent_target(
                owner.clone(),
                AgentVerb::CancelInvocation,
                AgentResourcePattern::InvocationId(invocation.clone()),
            ),
            agent_target(
                owner.clone(),
                AgentVerb::UpdateRevision,
                AgentResourcePattern::Empty,
            ),
            agent_target(owner.clone(), AgentVerb::Fork, AgentResourcePattern::Empty),
            agent_target(
                owner.clone(),
                AgentVerb::Revert,
                AgentResourcePattern::OplogIndex(42),
            ),
            agent_target(
                owner.clone(),
                AgentVerb::ActivatePlugin,
                AgentResourcePattern::PluginName(AgentPluginName("plugin-a".into())),
            ),
            agent_target(
                owner,
                AgentVerb::DeactivatePlugin,
                AgentResourcePattern::PluginName(AgentPluginName("plugin-b".into())),
            ),
        ];
        let expected = [
            (AgentVerb::View, AgentResourcePattern::Empty),
            (AgentVerb::Delete, AgentResourcePattern::Empty),
            (AgentVerb::Interrupt, AgentResourcePattern::Empty),
            (AgentVerb::Resume, AgentResourcePattern::Empty),
            (
                AgentVerb::CancelInvocation,
                AgentResourcePattern::InvocationId(invocation),
            ),
            (AgentVerb::UpdateRevision, AgentResourcePattern::Empty),
            (AgentVerb::Fork, AgentResourcePattern::Empty),
            (AgentVerb::Revert, AgentResourcePattern::OplogIndex(42)),
            (
                AgentVerb::ActivatePlugin,
                AgentResourcePattern::PluginName(AgentPluginName("plugin-a".into())),
            ),
            (
                AgentVerb::DeactivatePlugin,
                AgentResourcePattern::PluginName(AgentPluginName("plugin-b".into())),
            ),
        ];
        for (target, (verb, resource)) in cases.into_iter().zip(expected) {
            assert!(matches!(
                target,
                PermissionTarget::Agent(ClassPermissionTarget {
                    verb: Some(actual_verb),
                    resource: actual_resource,
                    ..
                }) if actual_verb == verb && actual_resource == resource
            ));
        }
    }

    #[test]
    fn tool_target_preserves_typed_command_and_argument_boundaries() {
        let target = tool_target(
            tool("search"),
            &["documents", "find"],
            &["--query=two words", "literal value", "-iv"],
        )
        .unwrap();

        assert!(matches!(
            target,
            PermissionTarget::Tool(ClassPermissionTarget {
                verb: Some(ToolVerb::Invoke),
                resource: ToolResourcePattern::Invocation(ToolInvocationPattern {
                    command_path: Some(path),
                    args,
                }),
                ..
            }) if path == vec![ToolIdentifier("documents".into()), ToolIdentifier("find".into())]
                && args == vec![
                    ToolArgPattern::LongFlag {
                        name: ToolIdentifier("query".into()),
                        value: Some(ToolValuePattern::Literal(ToolValueLiteral("two words".into()))),
                    },
                    ToolArgPattern::Positional(ToolValuePattern::Literal(ToolValueLiteral("literal value".into()))),
                    ToolArgPattern::ShortFlags { flags: vec!['i', 'v'], value: None },
                ]
        ));

        assert!(tool_target(tool("root"), &[], &["argument"]).is_ok());
        assert!(tool_target(tool("bad"), &["not.valid"], &[]).is_err());
    }
    #[test]
    fn paths_are_canonical_and_cannot_escape() {
        assert_eq!(
            CanonicalGuestPath::new("/a/./b/../c").unwrap().as_str(),
            "/a/c"
        );
        assert!(CanonicalGuestPath::new("/../secret").is_err());
        assert!(CanonicalGuestPath::new("relative").is_err());
    }
    #[test]
    fn descriptor_relative_paths_remain_guest_paths() {
        let root = CanonicalGuestPath::new("/data").unwrap();
        let path = root.resolve("dir/../file").unwrap();
        let target = filesystem_target(agent("x"), FilesystemVerb::Stat, &path);
        assert!(
            matches!(target, PermissionTarget::Filesystem(t) if t.resource == resource("filesystem", "/data/file").unwrap())
        );
        assert!(root.resolve("../secret").is_err());
        assert!(root.resolve("/secret").is_err());
    }
    #[test]
    fn network_and_http_are_canonical() {
        assert!(network_target("127.000.000.001", Some(80)).is_err());
        let h = http_target("HTTPS://Example.COM.:443/a/../b?q=1").unwrap();
        assert_eq!(
            (h.scheme.as_str(), h.host.as_str(), h.port, h.path.as_str()),
            ("https", "example.com", 443, "/b")
        );
        assert_eq!(
            h.permission,
            network_target("example.com", Some(443)).unwrap()
        );
        let websocket = websocket_target("WSS://Example.COM./socket").unwrap();
        assert_eq!(
            (websocket.host.as_str(), websocket.port),
            ("example.com", 443)
        );
    }

    #[test]
    fn parsed_network_grants_use_the_same_hostname_normalization_as_runtime_targets() {
        let grant = NetworkResourcePattern::parse_resource("Example.COM:443").unwrap();
        let target = http_target("https://example.com/").unwrap().permission;
        let PermissionTarget::Network(target) = target else {
            panic!("HTTP must produce a network target");
        };

        assert!(grant.subsumes(&target.resource));
    }

    #[test]
    fn parsed_network_grants_normalize_trailing_dot_like_runtime_targets() {
        let grant = NetworkResourcePattern::parse_resource("Example.COM.:443").unwrap();
        let target = http_target("https://example.com./").unwrap().permission;
        let PermissionTarget::Network(target) = target else {
            panic!("HTTP must produce a network target");
        };

        assert!(grant.subsumes(&target.resource));
    }

    #[test]
    fn malformed_network_and_urls_fail_closed() {
        assert!(network_target("::1", Some(80)).is_err());
        assert!(network_target("*.example.com", None).is_err());
        assert!(http_target("ftp://example.com/a").is_err());
        assert!(http_target("https://u:p@example.com").is_err());
    }
    #[test]
    fn key_and_object_scopes_are_distinct() {
        let e = env("prod");
        assert_ne!(
            kv_target(e.clone(), KvVerb::Read, "bucket", "key").unwrap(),
            kv_bucket_target(e.clone(), KvVerb::List, "bucket").unwrap()
        );
        assert_ne!(
            blob_target(e.clone(), BlobVerb::Read, "container", "object").unwrap(),
            blob_container_target(e, BlobVerb::List, "container").unwrap()
        );
    }
    #[test]
    fn malformed_family_resources_are_rejected() {
        assert!(env_target(agent("x"), "1BAD").is_err());
        assert!(secret_target(env("prod"), SecretVerb::Reveal, "a..b").is_err());
        assert!(config_segments_target(agent("x"), &["a.*x".into()]).is_err());
        assert!(oplog_target(agent("x"), Some(5), Some(4)).is_err());
    }

    #[test]
    fn kv_and_blob_targets_preserve_utf8_resource_names() {
        let owner = env("prod");
        assert!(matches!(
            kv_target(owner.clone(), KvVerb::Read, "store.name", "κλειδί.1").unwrap(),
            PermissionTarget::Kv(ClassPermissionTarget {
                resource: KvResourcePattern::StoreKey { store, key_pattern },
                ..
            }) if store == "store.name" && key_pattern == "κλειδί.1"
        ));
        assert!(matches!(
            blob_target(owner, BlobVerb::Read, "bucket.name", "αντικείμενο.1").unwrap(),
            PermissionTarget::Blob(ClassPermissionTarget {
                resource: BlobResourcePattern::BucketKey { bucket, key_pattern },
                ..
            }) if bucket == "bucket.name" && key_pattern == "αντικείμενο.1"
        ));
    }

    #[test]
    fn secret_inspection_and_reveal_use_distinct_verbs_for_the_same_exact_key() {
        let owner = env("prod");
        let hold = secret_target(owner.clone(), SecretVerb::Hold, "service.api-key").unwrap();
        let reveal = secret_target(owner, SecretVerb::Reveal, "service.api-key").unwrap();

        assert!(matches!(
            hold,
            PermissionTarget::Secret(ClassPermissionTarget {
                verb: Some(SecretVerb::Hold),
                ..
            })
        ));
        assert!(matches!(
            reveal,
            PermissionTarget::Secret(ClassPermissionTarget {
                verb: Some(SecretVerb::Reveal),
                ..
            })
        ));
    }

    #[test]
    fn config_keys_keep_typed_segments() {
        let target = config_segments_target(
            agent("x"),
            &["database".into(), "primary".into(), "url".into()],
        )
        .unwrap();
        assert!(matches!(
            target,
            PermissionTarget::Config(ClassPermissionTarget {
                resource: ConfigResourcePattern::Key(ConfigKeyPathPattern { segments }),
                ..
            }) if segments == vec![
                ConfigKeySegmentPattern::Literal("database".to_string()),
                ConfigKeySegmentPattern::Literal("primary".to_string()),
                ConfigKeySegmentPattern::Literal("url".to_string()),
            ]
        ));
        assert!(config_segments_target(agent("x"), &["database.primary".into()]).is_err());
    }

    #[test]
    fn oplog_targets_keep_typed_open_and_bounded_ranges() {
        let owner = agent("x");
        let open = oplog_target(owner.clone(), Some(7), None).unwrap();
        let bounded = oplog_target(owner, Some(7), Some(11)).unwrap();
        assert!(matches!(
            open,
            PermissionTarget::Oplog(ClassPermissionTarget {
                resource: OplogResourcePattern::Range {
                    start: Some(7),
                    end: None
                },
                ..
            })
        ));
        assert!(matches!(
            bounded,
            PermissionTarget::Oplog(ClassPermissionTarget {
                resource: OplogResourcePattern::Range {
                    start: Some(7),
                    end: Some(11)
                },
                ..
            })
        ));
    }

    fn tables(sql: &str) -> Vec<(RdbmsVerb, String, String, String)> {
        rdbms_sql_targets(env("prod"), RdbmsEngine::Postgres, "db", "public", sql)
            .unwrap()
            .into_iter()
            .map(|t| match t {
                PermissionTarget::Rdbms(ClassPermissionTarget {
                    verb: Some(v),
                    resource:
                        RdbmsResourcePattern::Table {
                            database,
                            schema,
                            table,
                        },
                    ..
                }) => (v, database, schema, table),
                _ => panic!(),
            })
            .collect()
    }
    #[test]
    fn sql_select_and_joins() {
        assert_eq!(
            tables("select * from users u join audit.events e on e.id=u.id"),
            vec![
                (
                    RdbmsVerb::Query,
                    "db".into(),
                    "public".into(),
                    "users".into()
                ),
                (
                    RdbmsVerb::Query,
                    "db".into(),
                    "audit".into(),
                    "events".into()
                )
            ]
        );
    }

    #[test]
    fn tableless_select_is_statically_extractable() {
        assert!(
            rdbms_sql_targets(env("prod"), RdbmsEngine::Mysql, "db", "db", "SELECT 1",).is_ok()
        );
    }

    #[test]
    fn sql_mutations_are_extracted() {
        assert_eq!(
            tables("insert into s.items values (1)")[0].0,
            RdbmsVerb::Mutate
        );
        assert_eq!(tables("insert into s.items(id) values (1)")[0].3, "items");
        assert_eq!(tables("update s.items set x=1")[0].3, "items");
        assert_eq!(
            tables("delete from s.items where x=1")[0].0,
            RdbmsVerb::Mutate
        );
        assert_eq!(tables("truncate table s.items")[0].3, "items");
    }

    #[test]
    fn sql_mutations_preflight_source_and_constraint_tables() {
        assert_eq!(
            tables("delete from current using archive where current.id = archive.id")
                .iter()
                .map(|target| target.3.as_str())
                .collect::<Vec<_>>(),
            vec!["current", "archive"]
        );
        assert_eq!(
            tables(
                "create table child (parent_id int references auth.parent(id) on update cascade)"
            )
            .iter()
            .map(|target| target.3.as_str())
            .collect::<Vec<_>>(),
            vec!["child", "parent"]
        );
    }

    #[test]
    fn alter_table_rename_preflights_the_destination_or_fails_closed() {
        let targets = rdbms_sql_targets(
            env("prod"),
            RdbmsEngine::Postgres,
            "db",
            "public",
            "ALTER TABLE source RENAME TO protected",
        );

        let tables = targets
            .unwrap()
            .into_iter()
            .map(|target| match target {
                PermissionTarget::Rdbms(ClassPermissionTarget {
                    verb: Some(verb),
                    resource: RdbmsResourcePattern::Table { table, .. },
                    ..
                }) => (verb, table),
                _ => panic!(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            tables,
            vec![
                (RdbmsVerb::Mutate, "source".to_string()),
                (RdbmsVerb::Mutate, "protected".to_string())
            ]
        );
    }

    #[test]
    fn alter_table_set_schema_preflights_the_destination_or_fails_closed() {
        let targets = rdbms_sql_targets(
            env("prod"),
            RdbmsEngine::Postgres,
            "db",
            "public",
            "ALTER TABLE source SET SCHEMA protected",
        );

        match targets {
            Ok(targets) => {
                let tables = targets
                    .into_iter()
                    .map(|target| match target {
                        PermissionTarget::Rdbms(ClassPermissionTarget {
                            verb: Some(verb),
                            resource: RdbmsResourcePattern::Table { schema, table, .. },
                            ..
                        }) => (verb, schema, table),
                        _ => panic!(),
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    tables,
                    vec![
                        (
                            RdbmsVerb::Mutate,
                            "public".to_string(),
                            "source".to_string()
                        ),
                        (
                            RdbmsVerb::Mutate,
                            "protected".to_string(),
                            "source".to_string()
                        )
                    ]
                );
            }
            Err(TargetError::SqlNotStaticallyExtractable(_)) => {}
            Err(error) => panic!("unexpected target error: {error}"),
        }
    }

    #[test]
    fn insert_select_requires_query_authority_for_the_source_or_fails_closed() {
        let targets = rdbms_sql_targets(
            env("prod"),
            RdbmsEngine::Postgres,
            "db",
            "public",
            "INSERT INTO destination SELECT * FROM protected",
        );

        let tables = targets
            .unwrap()
            .into_iter()
            .map(|target| match target {
                PermissionTarget::Rdbms(ClassPermissionTarget {
                    verb: Some(verb),
                    resource: RdbmsResourcePattern::Table { table, .. },
                    ..
                }) => (verb, table),
                _ => panic!(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            tables,
            vec![
                (RdbmsVerb::Mutate, "destination".to_string()),
                (RdbmsVerb::Query, "protected".to_string())
            ]
        );
    }

    #[test]
    fn create_table_like_requires_query_authority_for_the_source_or_fails_closed() {
        let targets = rdbms_sql_targets(
            env("prod"),
            RdbmsEngine::Postgres,
            "db",
            "public",
            "CREATE TABLE destination (LIKE protected INCLUDING ALL)",
        );

        match targets {
            Ok(targets) => {
                let tables = targets
                    .into_iter()
                    .map(|target| match target {
                        PermissionTarget::Rdbms(ClassPermissionTarget {
                            verb: Some(verb),
                            resource: RdbmsResourcePattern::Table { table, .. },
                            ..
                        }) => (verb, table),
                        _ => panic!(),
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    tables,
                    vec![
                        (RdbmsVerb::Mutate, "destination".to_string()),
                        (RdbmsVerb::Query, "protected".to_string())
                    ]
                );
            }
            Err(TargetError::SqlNotStaticallyExtractable(_)) => {}
            Err(error) => panic!("unexpected target error: {error}"),
        }
    }

    #[test]
    fn sql_comma_table_lists_are_fully_preflighted() {
        assert_eq!(
            tables("select x.id, y.id from a AS x, audit.b AS y where x.id = y.id")
                .iter()
                .map(|target| target.3.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert_eq!(
            tables("drop table first, archive.second")
                .iter()
                .map(|target| target.3.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
    }

    #[test]
    fn sql_nested_query_sources_require_query_authority() {
        let targets = tables(
            "UPDATE destination SET value = (SELECT value FROM protected) WHERE id IN (SELECT id FROM audit.visible)",
        );
        assert_eq!(
            targets
                .iter()
                .map(|target| (target.0, target.3.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (RdbmsVerb::Mutate, "destination"),
                (RdbmsVerb::Query, "protected"),
                (RdbmsVerb::Query, "visible"),
            ]
        );
    }

    #[test]
    fn sql_dialects_all_extract_static_table_references() {
        for (engine, sql, expected) in [
            (
                RdbmsEngine::Postgres,
                "SELECT * FROM \"audit\".\"events\"",
                ("db", "audit", "events"),
            ),
            (
                RdbmsEngine::Mysql,
                "SELECT * FROM `tenant`.`users`",
                ("tenant", "tenant", "users"),
            ),
            (
                RdbmsEngine::Ignite,
                "SELECT * FROM audit.events",
                ("db", "audit", "events"),
            ),
        ] {
            let targets = rdbms_sql_targets(env("prod"), engine, "db", "public", sql).unwrap();
            assert!(matches!(
                targets.as_slice(),
                [PermissionTarget::Rdbms(ClassPermissionTarget {
                    verb: Some(RdbmsVerb::Query),
                    resource: RdbmsResourcePattern::Table { database, schema, table },
                    ..
                })] if (database.as_str(), schema.as_str(), table.as_str()) == expected
            ));
        }
    }

    #[test]
    fn mysql_two_part_names_select_the_database_and_schema() {
        let targets = rdbms_sql_targets(
            env("prod"),
            RdbmsEngine::Mysql,
            "default_db",
            "default_db",
            "select * from tenant.users",
        )
        .unwrap();
        assert!(matches!(
            &targets[0],
            PermissionTarget::Rdbms(ClassPermissionTarget {
                resource: RdbmsResourcePattern::Table { database, schema, table },
                ..
            }) if database == "tenant" && schema == "tenant" && table == "users"
        ));
    }
    #[test]
    fn sql_qualified_quoted_and_subquery_tables() {
        assert_eq!(
            tables("SELECT * FROM \"Main\".\"User\" WHERE id IN (SELECT id FROM db2.audit.log)")
                .iter()
                .map(|x| x.3.as_str())
                .collect::<Vec<_>>(),
            vec!["User", "log"]
        );
    }
    #[test]
    fn sql_multi_table_is_preflighted() {
        assert_eq!(
            tables("select * from a join b on true join s.c on true").len(),
            3
        );
    }
    #[test]
    fn sql_conservative_behavior_is_explicit() {
        assert!(
            rdbms_sql_targets(
                env("prod"),
                RdbmsEngine::Mysql,
                "db",
                "s",
                "CALL dynamic_sql()",
            )
            .is_err()
        );
        assert!(
            rdbms_sql_targets(
                env("prod"),
                RdbmsEngine::Ignite,
                "db",
                "s",
                "CALL dynamic_sql()",
            )
            .is_err()
        );
        assert!(
            rdbms_sql_targets(
                env("prod"),
                RdbmsEngine::Postgres,
                "db",
                "s",
                "SELECT * FROM catalog.schema.table.extra",
            )
            .is_err()
        );
        assert!(
            rdbms_sql_targets(
                env("prod"),
                RdbmsEngine::Postgres,
                "db",
                "s",
                "select * from (values (1)) x",
            )
            .is_err()
        );
    }

    #[test]
    fn sql_batches_cannot_inherit_the_first_statements_verb() {
        assert!(
            rdbms_sql_targets(
                env("prod"),
                RdbmsEngine::Postgres,
                "db",
                "public",
                "SELECT * FROM visible; DROP TABLE protected",
            )
            .is_err()
        );
        assert_eq!(tables("SELECT * FROM visible;")[0].3, "visible");
    }

    #[test]
    fn sql_wrappers_with_ambiguous_effects_fail_closed() {
        for sql in [
            "WITH moved AS (DELETE FROM source RETURNING *) SELECT * FROM moved",
            "WITH visible AS (SELECT * FROM source) SELECT * FROM visible",
            "EXPLAIN DROP TABLE protected",
            "SELECT * INTO copied FROM source",
            "SELECT * FROM accounts FOR UPDATE",
            "SELECT * FROM dynamic_table(?)",
            "CREATE VIEW exposed AS SELECT * FROM protected",
            "CREATE INDEX idx ON protected(id)",
            "DROP TABLE first,",
            "SELECT * FROM catalog.schema.table.extra",
        ] {
            assert!(
                rdbms_sql_targets(env("prod"), RdbmsEngine::Postgres, "db", "public", sql,)
                    .is_err(),
                "accepted ambiguous SQL: {sql}"
            );
        }
    }
}
