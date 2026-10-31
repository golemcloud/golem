// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use golem_common::model::card::network_target::NetworkTargetError;
pub use golem_common::model::card::network_target::{
    http_target, network_target, websocket_target,
};
use golem_common::model::card::owner::{
    AgentOwnerLeafPattern, AgentOwnerPattern, EnvironmentOwnerPattern, ToolOwnerPattern,
};
use golem_common::model::card::*;
use sqlparser::ast::{
    AlterTableOperation, ArrayElemTypeDef, BinaryOperator, ColumnDef, ColumnOption,
    CreateTableLikeKind, DataType, Expr, FromTable, FunctionArguments, ObjectName, ObjectNamePart,
    ObjectType, Query, RenameTableNameKind, Select, SetExpr, Spanned, Statement, TableConstraint,
    TableFactor, TableObject, UnaryOperator, Value, Visit, Visitor,
};
use sqlparser::dialect::{GenericDialect, MySqlDialect, PostgreSqlDialect};
use sqlparser::keywords::Keyword;
use sqlparser::parser::{IsOptional, Parser};
use sqlparser::tokenizer::{Token, Tokenizer};
use std::fmt::{Display, Formatter};
use std::ops::ControlFlow;

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
    SqlNotStaticallyExtractable(String),
}

impl Display for TargetError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPath(v) => write!(f, "invalid guest path: {v}"),
            Self::InvalidResource { class, value } => {
                write!(f, "invalid {class} resource: {value}")
            }
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

pub fn dns_target(host: &str) -> Result<PermissionTarget, NetworkTargetError> {
    network_target(host, None)
}

pub fn tcp_target(host: &str, port: u16) -> Result<PermissionTarget, NetworkTargetError> {
    network_target(host, Some(port))
}

pub fn udp_target(host: &str, port: u16) -> Result<PermissionTarget, NetworkTargetError> {
    network_target(host, Some(port))
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
    args: Vec<ToolArgPattern<ToolInvocationValue>>,
    definition: Option<&golem_common::schema::tool::Tool>,
) -> Result<PermissionTarget, TargetError> {
    let parsed =
        ToolInvocationPattern::from_command_and_args(command_path, &[]).map_err(|value| {
            TargetError::InvalidResource {
                class: "tool",
                value,
            }
        })?;
    let normalized =
        parsed
            .normalize(definition)
            .map_err(|error| TargetError::InvalidResource {
                class: "tool",
                value: error.to_string(),
            })?;
    let mut invocation = ToolInvocation {
        command_path: normalized.command_path,
        flags: vec![],
        positionals: vec![],
    };
    for arg in args {
        match arg {
            ToolArgPattern::LongFlag { name, value } => {
                invocation.flags.push(CanonicalToolFlag { name, value })
            }
            ToolArgPattern::Positional(value) => invocation.positionals.push(value),
            ToolArgPattern::ShortFlags { .. } => {
                return Err(TargetError::InvalidResource {
                    class: "tool",
                    value: "invocation flags must use primary long names".into(),
                });
            }
        }
    }
    Ok(PermissionTarget::Tool(ClassPermissionTarget {
        verb: Some(ToolVerb::Invoke),
        owner,
        resource: ToolResourcePattern::Request(invocation),
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

pub struct RdbmsSqlAnalysis {
    pub targets: Vec<PermissionTarget>,
    /// One-based positions whose bound values must advertise the built-in text type.
    pub text_cast_parameters: Vec<usize>,
}

impl RdbmsSqlAnalysis {
    pub fn validate_parameters(
        &self,
        is_builtin_text: impl Fn(usize) -> bool,
    ) -> Result<(), TargetError> {
        validate_cast_parameters(&self.text_cast_parameters, is_builtin_text)
    }
}

fn validate_cast_parameters(
    indices: &[usize],
    is_builtin_text: impl Fn(usize) -> bool,
) -> Result<(), TargetError> {
    if indices.iter().all(|index| is_builtin_text(*index)) {
        Ok(())
    } else {
        Err(TargetError::SqlNotStaticallyExtractable(
            "trusted casts require a built-in text parameter at each referenced position"
                .to_string(),
        ))
    }
}

pub fn rdbms_sql_analysis(
    owner: EnvironmentOwnerPattern,
    engine: RdbmsEngine,
    database: &str,
    default_schema: &str,
    sql: &str,
) -> Result<RdbmsSqlAnalysis, TargetError> {
    let analysis = analyze_rdbms_statement(engine, sql)?;
    let targets = analysis
        .relations
        .into_iter()
        .map(|(name, verb)| {
            let parts = object_name_parts(&name, engine)?;
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
        .collect::<Result<Vec<_>, TargetError>>()?;
    Ok(RdbmsSqlAnalysis {
        targets,
        text_cast_parameters: analysis.text_cast_parameters,
    })
}

pub fn rdbms_sql_execution(
    engine: RdbmsEngine,
    sql: &str,
    is_builtin_text: impl Fn(usize) -> bool,
) -> Result<String, TargetError> {
    let analysis = analyze_rdbms_statement(engine, sql)?;
    validate_cast_parameters(&analysis.text_cast_parameters, is_builtin_text)?;
    Ok(analysis.executable_sql)
}

struct AnalyzedRdbmsStatement {
    relations: Vec<(ObjectName, RdbmsVerb)>,
    executable_sql: String,
    text_cast_parameters: Vec<usize>,
}

fn analyze_rdbms_statement(
    engine: RdbmsEngine,
    sql: &str,
) -> Result<AnalyzedRdbmsStatement, TargetError> {
    let mut executable_sql = qualify_trusted_dispatch(engine, sql)?;
    let sql = executable_sql.as_str();
    let statements = match engine {
        RdbmsEngine::Postgres => parse_postgres_sql(sql),
        RdbmsEngine::Mysql => Parser::parse_sql(&MySqlDialect {}, sql)
            .map(|statements| (statements, Vec::new()))
            .map_err(|error| error.to_string()),
        RdbmsEngine::Ignite => parse_ignite_sql(sql).map(|statements| (statements, Vec::new())),
    };
    let (mut statements, predicates) = match statements {
        Ok((statements, predicates)) if statements.len() == 1 => (statements, predicates),
        Ok(_) => {
            return Err(TargetError::SqlNotStaticallyExtractable(
                "exactly one statement is required".to_string(),
            ));
        }
        Err(error) => return Err(TargetError::SqlNotStaticallyExtractable(error)),
    };
    let mut cast_parameters = SqlCastParameters::default();
    let mut namespace = SqlNamespaceQualifier::default();
    for predicate in &predicates {
        if let ControlFlow::Break(reason) = predicate.visit(&mut SqlExpressionValidator { engine })
        {
            return Err(TargetError::SqlNotStaticallyExtractable(reason.to_string()));
        }
        let _ = predicate.visit(&mut cast_parameters);
        let _ = predicate.visit(&mut namespace);
    }
    let statement = statements.pop().expect("one parsed statement");
    let relations = match extract_sql_relations(&statement, engine) {
        Ok(relations) => relations,
        Err(reason) => return Err(TargetError::SqlNotStaticallyExtractable(reason)),
    };
    // The parser can mistake PostgreSQL's ONLY modifier for a relation with
    // an alias. Never turn that ambiguous token into an executable table name.
    if engine == RdbmsEngine::Postgres
        && relations.physical.iter().any(|name| {
            matches!(name.0.as_slice(), [ObjectNamePart::Identifier(id)]
                if id.quote_style.is_none() && id.value.eq_ignore_ascii_case("only"))
        })
    {
        return Err(TargetError::SqlNotStaticallyExtractable(
            "PostgreSQL ONLY relation modifiers are not statically analyzable".to_string(),
        ));
    }

    // Validate even when only the executable SQL is requested.
    for (name, _) in &relations.targets {
        object_name_parts(name, engine)?;
    }
    let _ = statement.visit(&mut cast_parameters);
    if engine == RdbmsEngine::Postgres {
        let _ = statement.visit(&mut namespace);
        executable_sql = namespace.qualify(&executable_sql, &relations.physical)?;
    }
    cast_parameters.0.sort_unstable();
    cast_parameters.0.dedup();
    Ok(AnalyzedRdbmsStatement {
        relations: relations.targets,
        executable_sql,
        text_cast_parameters: cast_parameters.0,
    })
}

#[derive(Default)]
struct SqlNamespaceQualifier {
    relation_lookups: Vec<(sqlparser::tokenizer::Span, Value)>,
}

impl Visitor for SqlNamespaceQualifier {
    type Break = ();

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        if let Expr::Cast {
            expr,
            data_type: DataType::Custom(name, _),
            ..
        } = expr
            && catalog_name(name, "regclass")
            && let Expr::Value(value) = expr.as_ref()
        {
            self.relation_lookups
                .push((expr.span(), value.value.clone()));
        }
        ControlFlow::Continue(())
    }
}

impl SqlNamespaceQualifier {
    fn qualify(self, sql: &str, physical_relations: &[ObjectName]) -> Result<String, TargetError> {
        let mut edits = Vec::new();
        for name in physical_relations {
            if let [ObjectNamePart::Identifier(id)] = name.0.as_slice() {
                let offset = sql_location_offset(sql, id.span.start)?;
                edits.push((offset, offset, "public.".to_string()));
            }
        }
        for (span, value) in self.relation_lookups {
            let start = sql_location_offset(sql, span.start)?;
            let end = sql_location_offset(sql, span.end)?;
            let source = &sql[start..end];
            let replacement = match value {
                // Keep parameters bound, including when the same placeholder
                // occurs elsewhere. Preserve PostgreSQL's regclass name grammar,
                // which also accepts names such as migration-log without quotes.
                Value::Placeholder(_) => format!(
                    "(CASE WHEN pg_catalog.regexp_match({source}, E'^[ \\t\\n\\r\\013\\014]*(?:\"(?:[^\"]|\"\")*\"[^.]*|[^\". \\t\\n\\r\\013\\014][^.]*)$') IS NOT NULL THEN pg_catalog.format('public.%s', {source}) ELSE {source} END)"
                ),
                // Keep literal casts constant so PostgreSQL can bind their OIDs
                // at parse time, including in indexes and stored constraints.
                Value::SingleQuotedString(value) if !value.contains('\\') => {
                    qualify_regclass_literal(&value, source)?
                }
                Value::EscapedStringLiteral(value) => qualify_regclass_literal(&value, source)?,
                Value::DollarQuotedString(value) => qualify_regclass_literal(&value.value, source)?,
                _ => {
                    return Err(TargetError::SqlNotStaticallyExtractable(
                        "unsupported regclass literal".to_string(),
                    ));
                }
            };
            edits.push((start, end, replacement));
        }
        edits.sort_unstable();
        edits.dedup();
        let mut result = sql.to_string();
        for (start, end, replacement) in edits.into_iter().rev() {
            result.replace_range(start..end, &replacement);
        }
        Ok(result)
    }
}

fn qualify_regclass_literal(value: &str, source: &str) -> Result<String, TargetError> {
    // regclass treats a quote as a delimiter only at the start of a name part.
    // Match PostgreSQL scanner_isspace, not Unicode or locale whitespace.
    let name = value.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let mut quoted = name.starts_with('"');
    let mut chars = name.chars().peekable();
    if quoted {
        chars.next();
    }
    let mut qualified = false;
    while let Some(ch) = chars.next() {
        if quoted && ch == '"' {
            if chars.next_if_eq(&'"').is_none() {
                quoted = false;
            }
        } else if ch == '.' && !quoted {
            qualified = true;
        }
    }
    if quoted {
        return Err(TargetError::SqlNotStaticallyExtractable(
            "unclosed quote in relation-name literal".to_string(),
        ));
    }
    if !qualified {
        Ok(format!(
            "E'public.{}'",
            value.replace('\\', "\\\\").replace('\'', "''")
        ))
    } else {
        Ok(source.to_string())
    }
}

fn sql_location_offset(
    sql: &str,
    location: sqlparser::tokenizer::Location,
) -> Result<usize, TargetError> {
    let mut line = 1;
    let mut column = 1;
    for (offset, ch) in sql.char_indices().chain(std::iter::once((sql.len(), '\0'))) {
        if line == location.line && column == location.column {
            return Ok(offset);
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    Err(TargetError::SqlNotStaticallyExtractable(
        "cannot locate physical SQL reference".to_string(),
    ))
}

// Only inserts a schema qualifier at token boundaries. Strings, comments,
// parameters, conflict predicates and all unrelated SQL bytes remain unchanged.
fn qualify_trusted_dispatch(engine: RdbmsEngine, sql: &str) -> Result<String, TargetError> {
    if engine == RdbmsEngine::Mysql {
        return qualify_mysql_now(sql);
    }
    if engine != RdbmsEngine::Postgres {
        return Ok(sql.to_string());
    }
    let tokens = Tokenizer::new(&PostgreSqlDialect {}, sql)
        .tokenize_with_location()
        .map_err(|error| TargetError::SqlNotStaticallyExtractable(error.to_string()))?
        .into_iter()
        .filter(|token| !matches!(token.token, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    let mut insertions = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let Token::Word(word) = &token.token else {
            continue;
        };
        let name = if word.quote_style.is_none() {
            word.value.to_ascii_lowercase()
        } else {
            word.value.clone()
        };
        let previous = index.checked_sub(1).map(|index| &tokens[index].token);
        let next = tokens.get(index + 1).map(|token| &token.token);
        let regclass = matches!(name.as_str(), "regclass" | "uuid")
            && previous == Some(&Token::DoubleColon)
            && next != Some(&Token::Period);
        if regclass {
            let location = token.span.start;
            let mut line = 1;
            let mut column = 1;
            let offset = sql
                .char_indices()
                .find_map(|(offset, ch)| {
                    if line == location.line && column == location.column {
                        return Some(offset);
                    }
                    if ch == '\n' {
                        line += 1;
                        column = 1;
                    } else {
                        column += 1;
                    }
                    None
                })
                .ok_or_else(|| {
                    TargetError::SqlNotStaticallyExtractable(
                        "cannot locate trusted SQL token".to_string(),
                    )
                })?;
            insertions.push(offset);
        }
    }
    let mut executable = sql.to_string();
    for offset in insertions.into_iter().rev() {
        executable.insert_str(offset, "pg_catalog.");
    }
    let (statements, predicates) =
        parse_postgres_sql(&executable).map_err(TargetError::SqlNotStaticallyExtractable)?;
    let mut qualifier = TrustedDispatchQualifier {
        engine,
        locations: Vec::new(),
        tokens: Tokenizer::new(&PostgreSqlDialect {}, &executable)
            .tokenize_with_location()
            .map_err(|error| TargetError::SqlNotStaticallyExtractable(error.to_string()))?
            .into_iter()
            .filter(|token| !matches!(token.token, Token::Whitespace(_)))
            .collect(),
    };
    for statement in &statements {
        let _ = statement.visit(&mut qualifier);
    }
    for predicate in &predicates {
        let _ = predicate.visit(&mut qualifier);
    }
    let mut offsets = qualifier
        .locations
        .into_iter()
        .map(|location| {
            let mut line = 1;
            let mut column = 1;
            executable
                .char_indices()
                .find_map(|(offset, ch)| {
                    if line == location.line && column == location.column {
                        return Some(offset);
                    }
                    if ch == '\n' {
                        line += 1;
                        column = 1;
                    } else {
                        column += 1;
                    }
                    None
                })
                .ok_or_else(|| {
                    TargetError::SqlNotStaticallyExtractable(
                        "cannot locate trusted function".to_string(),
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    offsets.sort_unstable();
    offsets.dedup();
    for offset in offsets.into_iter().rev() {
        executable.insert_str(offset, "pg_catalog.");
    }
    Ok(executable)
}

fn qualify_mysql_now(sql: &str) -> Result<String, TargetError> {
    let statements = Parser::parse_sql(&MySqlDialect {}, sql)
        .map_err(|error| TargetError::SqlNotStaticallyExtractable(error.to_string()))?;
    let mut qualifier = TrustedDispatchQualifier {
        engine: RdbmsEngine::Mysql,
        locations: Vec::new(),
        tokens: Vec::new(),
    };
    for statement in statements {
        let _ = statement.visit(&mut qualifier);
    }
    let mut offsets = qualifier
        .locations
        .into_iter()
        .map(|location| {
            let mut line = 1;
            let mut column = 1;
            sql.char_indices()
                .find_map(|(offset, ch)| {
                    if line == location.line && column == location.column {
                        return Some(offset);
                    }
                    if ch == '\n' {
                        line += 1;
                        column = 1;
                    } else {
                        column += 1;
                    }
                    None
                })
                .ok_or_else(|| {
                    TargetError::SqlNotStaticallyExtractable(
                        "cannot locate trusted SQL token".to_string(),
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    offsets.sort_unstable();
    offsets.dedup();
    let mut executable = sql.to_string();
    for offset in offsets.into_iter().rev() {
        executable.replace_range(offset..offset + 3, "CURRENT_TIMESTAMP");
    }
    Ok(executable)
}

struct TrustedDispatchQualifier {
    engine: RdbmsEngine,
    locations: Vec<sqlparser::tokenizer::Location>,
    tokens: Vec<sqlparser::tokenizer::TokenWithSpan>,
}

impl TrustedDispatchQualifier {
    fn column(&mut self, column: &ColumnDef) {
        if self.engine != RdbmsEngine::Postgres {
            return;
        }
        let mut data_type = &column.data_type;
        while let DataType::Array(ArrayElemTypeDef::SquareBracket(element, _)) = data_type {
            data_type = element;
        }
        let name = match data_type {
            DataType::Text => "text",
            DataType::Date => "date",
            DataType::Uuid => "uuid",
            _ => return,
        };
        if let Some(token) = self.tokens.iter().find(|token| {
            (token.span.start.line, token.span.start.column)
                >= (column.name.span.end.line, column.name.span.end.column)
        }) && matches!(&token.token, Token::Word(word) if word.quote_style.is_none() && word.value.eq_ignore_ascii_case(name))
        {
            self.locations.push(token.span.start);
        }
    }
}

impl Visitor for TrustedDispatchQualifier {
    type Break = ();

    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<Self::Break> {
        match statement {
            Statement::CreateTable(create) => {
                for column in &create.columns {
                    self.column(column);
                }
            }
            Statement::AlterTable(alter) => {
                for operation in &alter.operations {
                    if let AlterTableOperation::AddColumn { column_def, .. } = operation {
                        self.column(column_def);
                    }
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        if let Expr::Function(function) = expr
            && let [ObjectNamePart::Identifier(name)] = function.name.0.as_slice()
            && ((self.engine == RdbmsEngine::Postgres
                && sql_identifier(name, self.engine) == "now")
                || (self.engine == RdbmsEngine::Mysql && name.value.eq_ignore_ascii_case("now")))
            && (self.engine == RdbmsEngine::Postgres || name.quote_style.is_none())
        {
            self.locations.push(name.span.start);
        }
        ControlFlow::Continue(())
    }
}

fn parse_postgres_sql(sql: &str) -> Result<(Vec<Statement>, Vec<Expr>), String> {
    let dialect = PostgreSqlDialect {};
    let original_error = match Parser::parse_sql(&dialect, sql) {
        Ok(statements) => return Ok((statements, Vec::new())),
        Err(error) => error.to_string(),
    };
    let mut tokens = Tokenizer::new(&dialect, sql)
        .tokenize_with_location()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|token| !matches!(token.token, Token::Whitespace(_)))
        .collect::<Vec<_>>();
    let is_keyword = |token: &Token, keyword| matches!(token, Token::Word(word) if word.quote_style.is_none() && word.keyword == keyword);
    let mut extended = false;
    let mut predicates = Vec::new();
    let mut index = 0;
    while index + 2 < tokens.len() {
        if !is_keyword(&tokens[index].token, Keyword::ON)
            || !is_keyword(&tokens[index + 1].token, Keyword::CONFLICT)
            || tokens[index + 2].token != Token::LParen
        {
            index += 1;
            continue;
        }
        let start = index + 2;
        let mut parser = Parser::new(&dialect).with_tokens_with_locations(tokens[start..].to_vec());
        parser
            .parse_parenthesized_column_list(IsOptional::Mandatory, false)
            .map_err(|error| error.to_string())?;
        if !is_keyword(&parser.next_token().token, Keyword::WHERE) {
            index += 1;
            continue;
        }
        let predicate_start = start + parser.get_current_index();
        let predicate = parser.parse_expr().map_err(|error| error.to_string())?;
        if let ControlFlow::Break(reason) = predicate.visit(&mut ConflictPredicateValidator) {
            return Err(reason.to_string());
        }
        predicates.push(predicate);
        if !is_keyword(&parser.next_token().token, Keyword::DO) {
            return Err("conflict-target predicate must be followed by DO".to_string());
        }
        let predicate_end = start + parser.get_current_index();
        // sqlparser's conflict-target AST has no predicate. Omit only the parsed
        // scalar predicate for target analysis; execution still uses the original
        // SQL. Subqueries are forbidden so this cannot hide table access or writes.
        tokens.drain(predicate_start..predicate_end);
        extended = true;
        index += 1;
    }
    if !extended {
        return Err(original_error);
    }
    Parser::new(&dialect)
        .with_tokens_with_locations(tokens)
        .parse_statements()
        .map(|statements| (statements, predicates))
        .map_err(|error| error.to_string())
}

struct ConflictPredicateValidator;

impl Visitor for ConflictPredicateValidator {
    type Break = &'static str;

    fn pre_visit_query(&mut self, _query: &Query) -> ControlFlow<Self::Break> {
        ControlFlow::Break("subqueries in conflict-target predicates are unsupported")
    }
}

fn parse_ignite_sql(sql: &str) -> Result<Vec<Statement>, String> {
    let dialect = GenericDialect {};
    match Parser::parse_sql(&dialect, sql) {
        Ok(statements) => Ok(statements),
        Err(original_error) => {
            let mut tokens = Tokenizer::new(&dialect, sql)
                .tokenize()
                .map_err(|error| error.to_string())?;
            let first = tokens
                .iter_mut()
                .find(|token| !matches!(token, Token::Whitespace(_)))
                .ok_or_else(|| original_error.to_string())?;
            match first {
                Token::Word(word)
                    if word.quote_style.is_none() && word.keyword == Keyword::MERGE =>
                {
                    word.value = "INSERT".to_string();
                    word.keyword = Keyword::INSERT;
                }
                _ => return Err(original_error.to_string()),
            }

            let statements = Parser::new(&dialect)
                .with_tokens(tokens)
                .parse_statements()
                .map_err(|error| error.to_string())?;
            let [Statement::Insert(insert)] = statements.as_slice() else {
                return Err("Ignite MERGE shorthand must contain exactly one statement".to_string());
            };
            if !insert.into
                || !matches!(insert.table, TableObject::TableName(_))
                || insert.source.is_none()
                || !insert.optimizer_hints.is_empty()
                || insert.or.is_some()
                || insert.ignore
                || insert.table_alias.is_some()
                || insert.overwrite
                || !insert.assignments.is_empty()
                || insert.partitioned.is_some()
                || !insert.after_columns.is_empty()
                || insert.has_table_keyword
                || insert.on.is_some()
                || insert.returning.is_some()
                || insert.output.is_some()
                || insert.replace_into
                || insert.priority.is_some()
                || insert.insert_alias.is_some()
                || insert.settings.is_some()
                || insert.format_clause.is_some()
                || insert.multi_table_insert_type.is_some()
                || !insert.multi_table_into_clauses.is_empty()
                || !insert.multi_table_when_clauses.is_empty()
                || insert.multi_table_else_clause.is_some()
            {
                return Err("unsupported Ignite MERGE shorthand form".to_string());
            }
            Ok(statements)
        }
    }
}

struct SqlRelations {
    targets: Vec<(ObjectName, RdbmsVerb)>,
    physical: Vec<ObjectName>,
}

fn extract_sql_relations(
    statement: &Statement,
    engine: RdbmsEngine,
) -> Result<SqlRelations, String> {
    validate_sql_ast(statement, engine)?;
    let mut collector = SqlRelationCollector {
        engine,
        mutations: Vec::new(),
        queries: Vec::new(),
        relations: Vec::new(),
        ctes: Vec::new(),
        destinations: Vec::new(),
        suppressed_queries: 0,
        physical_relations: Vec::new(),
    };
    match statement.visit(&mut collector) {
        ControlFlow::Break(reason) => return Err(reason),
        ControlFlow::Continue(()) => {}
    }
    let mut result = collector.mutations;
    for name in collector.relations {
        if !result.iter().any(|(mutation, _)| mutation == &name) {
            push_unique_relation(&mut collector.queries, name, RdbmsVerb::Query);
        }
    }
    for (name, verb) in collector.queries {
        push_unique_relation(&mut result, name, verb);
    }
    Ok(SqlRelations {
        targets: result,
        physical: collector.physical_relations,
    })
}

fn statement_sql_relations(
    statement: &Statement,
    engine: RdbmsEngine,
    physical_relations: &mut Vec<ObjectName>,
) -> Result<Vec<(ObjectName, RdbmsVerb)>, String> {
    let mut mutations = Vec::new();
    let mut queries = Vec::new();
    let mut rename_destinations = Vec::new();
    match statement {
        Statement::Query(_) => {}
        Statement::Insert(insert) => {
            let TableObject::TableName(name) = &insert.table else {
                return Err("insert destination is not a concrete table".to_string());
            };
            push_unique(&mut mutations, name.clone());
        }
        Statement::Update(update) => {
            // A joined UPDATE can write any concrete table in its destination.
            for factor in std::iter::once(&update.table.relation)
                .chain(update.table.joins.iter().map(|join| &join.relation))
            {
                push_unique(&mut mutations, plain_table_factor_name(factor)?.clone());
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
        }
        Statement::Merge(merge) => {
            push_unique(
                &mut mutations,
                plain_table_factor_name(&merge.table)?.clone(),
            );
        }
        Statement::Truncate(truncate) => {
            for table in &truncate.table_names {
                push_unique(&mut mutations, table.name.clone());
            }
        }
        Statement::CreateTable(create) => {
            if engine == RdbmsEngine::Postgres && create.temporary {
                return Err("temporary table creation requires unanalyzed authority".to_string());
            }
            push_unique(&mut mutations, create.name.clone());
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
        Statement::CreateIndex(create) => {
            if !create.alter_options.is_empty() {
                return Err("unsupported create-index alter options".to_string());
            }
            push_unique(&mut mutations, create.table_name.clone());
        }
        Statement::Lock(lock) => {
            for table in &lock.tables {
                push_unique(&mut mutations, table.name.clone());
            }
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
                        let destination = if engine == RdbmsEngine::Postgres {
                            if name.0.len() != 1 {
                                return Err("rename destination must be unqualified".to_string());
                            }
                            let mut destination = alter.name.clone();
                            destination.0.pop();
                            destination.0.extend(name.0.clone());
                            destination
                        } else {
                            name.clone()
                        };
                        rename_destinations.push(destination);
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

    physical_relations.extend(mutations.iter().chain(&queries).cloned());
    mutations.extend(rename_destinations);
    let mut result = Vec::new();
    for name in mutations {
        push_unique_relation(&mut result, name, RdbmsVerb::Mutate);
    }
    for name in queries {
        push_unique_relation(&mut result, name, RdbmsVerb::Query);
    }
    Ok(result)
}

struct SqlRelationCollector {
    engine: RdbmsEngine,
    mutations: Vec<(ObjectName, RdbmsVerb)>,
    queries: Vec<(ObjectName, RdbmsVerb)>,
    relations: Vec<ObjectName>,
    ctes: Vec<String>,
    // AST node identities distinguish a write destination from a separate read
    // of the same table. The AST remains borrowed and unchanged during the walk.
    destinations: Vec<usize>,
    suppressed_queries: usize,
    physical_relations: Vec<ObjectName>,
}

fn sql_identifier(identifier: &sqlparser::ast::Ident, engine: RdbmsEngine) -> String {
    match engine {
        // MySQL CTE names are case-insensitive, including quoted names.
        RdbmsEngine::Mysql => identifier.value.to_lowercase(),
        _ if identifier.quote_style.is_some() => identifier.value.clone(),
        RdbmsEngine::Postgres => identifier.value.to_ascii_lowercase(),
        RdbmsEngine::Ignite => identifier.value.to_uppercase(),
    }
}

impl SqlRelationCollector {
    fn is_cte(&self, name: &ObjectName) -> bool {
        matches!(name.0.as_slice(), [ObjectNamePart::Identifier(id)] if self.ctes.contains(&sql_identifier(id, self.engine)))
    }

    fn query(&mut self, query: &Query) -> ControlFlow<String> {
        let inherited = self.ctes.clone();
        if let Some(with) = &query.with {
            let mut aliases = Vec::new();
            for cte in &with.cte_tables {
                let alias = sql_identifier(&cte.alias.name, self.engine);
                if cte.from.is_some() || aliases.contains(&alias) {
                    return ControlFlow::Break("ambiguous CTE definition".to_string());
                }
                aliases.push(alias);
            }
            if with.recursive {
                self.ctes.extend(aliases.clone());
            }
            for (cte, alias) in with.cte_tables.iter().zip(aliases) {
                cte.query.visit(self)?;
                if !with.recursive {
                    self.ctes.push(alias);
                }
            }
        }
        self.row_locks(query)?;
        query.body.visit(self)?;
        query.order_by.visit(self)?;
        query.limit_clause.visit(self)?;
        query.fetch.visit(self)?;
        self.ctes = inherited;
        ControlFlow::Continue(())
    }

    fn row_locks(&mut self, query: &Query) -> ControlFlow<String> {
        if query.locks.is_empty() {
            return ControlFlow::Continue(());
        }
        let SetExpr::Select(select) = query.body.as_ref() else {
            return ControlFlow::Break("row locks require a plain SELECT".to_string());
        };
        // Only direct, concrete FROM items can be resolved without propagating locks
        // through derived tables or accidentally locking tables inside a CTE.
        let alias_identifier = |id: &sqlparser::ast::Ident| {
            if self.engine == RdbmsEngine::Mysql {
                // MySQL table aliases can be case-sensitive, unlike CTE names.
                // Require the exact spelling rather than depend on server settings.
                id.value.clone()
            } else {
                sql_identifier(id, self.engine)
            }
        };
        let mut bindings = Vec::new();
        for table in &select.from {
            for factor in std::iter::once(&table.relation)
                .chain(table.joins.iter().map(|join| &join.relation))
            {
                let TableFactor::Table {
                    name,
                    alias,
                    args: None,
                    ..
                } = factor
                else {
                    return ControlFlow::Break("ambiguous row-lock table expression".to_string());
                };
                let id = match alias {
                    Some(alias) => &alias.name,
                    None => match name.0.last() {
                        Some(ObjectNamePart::Identifier(id)) => id,
                        _ => return ControlFlow::Break("invalid row-lock table name".to_string()),
                    },
                };
                bindings.push((alias_identifier(id), name));
            }
        }
        for lock in &query.locks {
            if let Some(of) = &lock.of {
                let [ObjectNamePart::Identifier(id)] = of.0.as_slice() else {
                    return ControlFlow::Break(
                        "row-lock OF requires an unqualified alias".to_string(),
                    );
                };
                let matches = bindings
                    .iter()
                    .filter(|(alias, _)| *alias == alias_identifier(id))
                    .collect::<Vec<_>>();
                let [(_, name)] = matches.as_slice() else {
                    return ControlFlow::Break("unknown or ambiguous row-lock alias".to_string());
                };
                if self.is_cte(name) {
                    return ControlFlow::Break("row-lock OF cannot target a CTE".to_string());
                }
                push_unique_relation(&mut self.mutations, (*name).clone(), RdbmsVerb::Mutate);
            } else {
                for (_, name) in &bindings {
                    // PostgreSQL does not propagate an outer lock into a CTE.
                    if !self.is_cte(name) {
                        push_unique_relation(
                            &mut self.mutations,
                            (*name).clone(),
                            RdbmsVerb::Mutate,
                        );
                    }
                }
            }
        }
        ControlFlow::Continue(())
    }
}

impl Visitor for SqlRelationCollector {
    type Break = String;

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<Self::Break> {
        if self.suppressed_queries == 0 {
            self.query(query)?;
        }
        // Query children were visited explicitly to give each CTE the correct
        // preceding definitions. Suppress the visitor's automatic second walk.
        self.suppressed_queries += 1;
        ControlFlow::Continue(())
    }

    fn post_visit_query(&mut self, _query: &Query) -> ControlFlow<Self::Break> {
        self.suppressed_queries -= 1;
        ControlFlow::Continue(())
    }

    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<Self::Break> {
        if self.suppressed_queries != 0 {
            return ControlFlow::Continue(());
        }
        let relations =
            match statement_sql_relations(statement, self.engine, &mut self.physical_relations) {
                Ok(relations) => relations,
                Err(reason) => return ControlFlow::Break(reason),
            };
        for (name, verb) in relations {
            let result = if verb == RdbmsVerb::Mutate {
                &mut self.mutations
            } else {
                &mut self.queries
            };
            push_unique_relation(result, name, verb);
        }
        if let Statement::Update(update) = statement {
            for factor in std::iter::once(&update.table.relation)
                .chain(update.table.joins.iter().map(|join| &join.relation))
            {
                self.destinations.push(std::ptr::from_ref(factor) as usize);
            }
        }
        let destination = match statement {
            Statement::Delete(delete) => {
                let (FromTable::WithFromKeyword(from) | FromTable::WithoutKeyword(from)) =
                    &delete.from;
                from.first().map(|table| &table.relation)
            }
            Statement::Merge(merge) => Some(&merge.table),
            _ => None,
        };
        if let Some(destination) = destination {
            self.destinations
                .push(std::ptr::from_ref(destination) as usize);
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_relation(&mut self, name: &ObjectName) -> ControlFlow<Self::Break> {
        if self.suppressed_queries == 0 && !self.is_cte(name) {
            self.physical_relations.push(name.clone());
            push_unique(&mut self.relations, name.clone());
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, factor: &TableFactor) -> ControlFlow<Self::Break> {
        if self.suppressed_queries == 0
            && !self
                .destinations
                .contains(&(std::ptr::from_ref(factor) as usize))
            && let TableFactor::Table { name, .. } = factor
            && !self.is_cte(name)
        {
            push_unique_relation(&mut self.queries, name.clone(), RdbmsVerb::Query);
        }
        ControlFlow::Continue(())
    }
}

fn validate_sql_ast(statement: &Statement, engine: RdbmsEngine) -> Result<(), String> {
    if let ControlFlow::Break(reason) = statement.visit(&mut SqlExpressionValidator { engine }) {
        return Err(reason.to_string());
    }
    let mut validator = SqlAstValidator;
    match statement.visit(&mut validator) {
        ControlFlow::Continue(()) => Ok(()),
        ControlFlow::Break(reason) => Err(reason.to_string()),
    }
}

struct SqlAstValidator;

struct SqlExpressionValidator {
    engine: RdbmsEngine,
}

fn catalog_name(name: &ObjectName, expected: &str) -> bool {
    matches!(name.0.as_slice(), [ObjectNamePart::Identifier(schema), ObjectNamePart::Identifier(name)]
        if sql_identifier(schema, RdbmsEngine::Postgres) == "pg_catalog"
            && sql_identifier(name, RdbmsEngine::Postgres) == expected)
}

fn text_cast_source(expr: &Expr) -> Result<Option<usize>, ()> {
    let Expr::Value(value) = expr else {
        return Err(());
    };
    match &value.value {
        Value::SingleQuotedString(_)
        | Value::EscapedStringLiteral(_)
        | Value::DollarQuotedString(_) => Ok(None),
        Value::Placeholder(name) => {
            let index = name.strip_prefix('$').ok_or(())?;
            if index.is_empty() || !index.bytes().all(|ch| ch.is_ascii_digit()) {
                return Err(());
            }
            let index = index.parse::<usize>().map_err(|_| ())?;
            if index == 0 { Err(()) } else { Ok(Some(index)) }
        }
        _ => Err(()),
    }
}

#[derive(Default)]
struct SqlCastParameters(Vec<usize>);

impl Visitor for SqlCastParameters {
    type Break = ();

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        if let Expr::Cast { expr, .. } = expr
            && let Ok(Some(index)) = text_cast_source(expr)
        {
            self.0.push(index);
        }
        ControlFlow::Continue(())
    }
}

fn approved_column_type(engine: RdbmsEngine, data_type: &DataType) -> bool {
    if engine == RdbmsEngine::Postgres
        && let DataType::Array(ArrayElemTypeDef::SquareBracket(element, _)) = data_type
    {
        return approved_column_type(engine, element);
    }
    matches!(
        data_type,
        DataType::Int(_)
            | DataType::Integer(_)
            | DataType::SmallInt(_)
            | DataType::BigInt(_)
            | DataType::Boolean
            | DataType::Varchar(_)
            | DataType::Char(_)
            | DataType::Character(_)
            | DataType::Timestamp(
                _,
                sqlparser::ast::TimezoneInfo::None
                    | sqlparser::ast::TimezoneInfo::WithTimeZone
                    | sqlparser::ast::TimezoneInfo::WithoutTimeZone
            )
            | DataType::Time(
                _,
                sqlparser::ast::TimezoneInfo::None
                    | sqlparser::ast::TimezoneInfo::WithTimeZone
                    | sqlparser::ast::TimezoneInfo::WithoutTimeZone
            )
            | DataType::Numeric(_)
            | DataType::Decimal(_)
            | DataType::Real
            | DataType::DoublePrecision
    ) || (engine != RdbmsEngine::Postgres && matches!(data_type, DataType::Text | DataType::Date))
        || (engine == RdbmsEngine::Postgres
            && matches!(data_type, DataType::Custom(name, modifiers)
          if (catalog_name(name, "text") || catalog_name(name, "date") || catalog_name(name, "uuid")) && modifiers.is_empty()))
}

fn explicit_index_dispatch(
    index_type: &Option<sqlparser::ast::IndexType>,
    options: &[sqlparser::ast::IndexOption],
    columns: &[sqlparser::ast::IndexColumn],
) -> bool {
    index_type.is_some()
        || options
            .iter()
            .any(|option| matches!(option, sqlparser::ast::IndexOption::Using(_)))
        || columns.iter().any(|column| column.operator_class.is_some())
}

fn constraint_selects_implementation(constraint: &TableConstraint) -> bool {
    match constraint {
        TableConstraint::PrimaryKey(constraint) => explicit_index_dispatch(
            &constraint.index_type,
            &constraint.index_options,
            &constraint.columns,
        ),
        TableConstraint::Unique(constraint) => explicit_index_dispatch(
            &constraint.index_type,
            &constraint.index_options,
            &constraint.columns,
        ),
        TableConstraint::Index(constraint) => explicit_index_dispatch(
            &constraint.index_type,
            &constraint.index_options,
            &constraint.columns,
        ),
        TableConstraint::FulltextOrSpatial(_)
        | TableConstraint::PrimaryKeyUsingIndex(_)
        | TableConstraint::UniqueUsingIndex(_) => true,
        TableConstraint::ForeignKey(_) | TableConstraint::Check(_) => false,
    }
}

fn column_selects_implementation(column: &ColumnDef) -> bool {
    column.options.iter().any(|option| match &option.option {
        ColumnOption::PrimaryKey(constraint) => explicit_index_dispatch(
            &constraint.index_type,
            &constraint.index_options,
            &constraint.columns,
        ),
        ColumnOption::Unique(constraint) => explicit_index_dispatch(
            &constraint.index_type,
            &constraint.index_options,
            &constraint.columns,
        ),
        ColumnOption::Collation(_) | ColumnOption::Policy(_) | ColumnOption::Options(_) => true,
        _ => false,
    })
}

impl Visitor for SqlExpressionValidator {
    type Break = &'static str;

    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<Self::Break> {
        let approved = |column: &ColumnDef| {
            approved_column_type(self.engine, &column.data_type)
                && !column_selects_implementation(column)
        };
        if let Statement::CreateIndex(create) = statement
            && (explicit_index_dispatch(&create.using, &create.index_options, &create.columns)
                || !create.with.is_empty())
        {
            return ControlFlow::Break(
                "explicit index access methods and operator classes are not in the trusted subset",
            );
        }
        let invalid = match statement {
            Statement::CreateTable(create) => {
                let table_options = match &create.table_options {
                    sqlparser::ast::CreateTableOptions::None => &[][..],
                    sqlparser::ast::CreateTableOptions::With(options)
                    | sqlparser::ast::CreateTableOptions::Options(options)
                    | sqlparser::ast::CreateTableOptions::Plain(options)
                    | sqlparser::ast::CreateTableOptions::TableProperties(options) => {
                        options.as_slice()
                    }
                };
                create.columns.iter().any(|column| !approved(column))
                    || create
                        .constraints
                        .iter()
                        .any(constraint_selects_implementation)
                    || create.hive_formats.is_some()
                    || create.external
                    || create.file_format.is_some()
                    || create.location.is_some()
                    || create.with_aggregation_policy.is_some()
                    || create.with_row_access_policy.is_some()
                    || create.with_storage_lifecycle_policy.is_some()
                    || table_options
                        .iter()
                        .any(|option| !matches!(option, sqlparser::ast::SqlOption::Comment(_)))
            }
            Statement::AlterTable(alter) => {
                alter.operations.iter().any(|operation| match operation {
                    AlterTableOperation::AddConstraint { constraint, .. } => {
                        constraint_selects_implementation(constraint)
                    }
                    AlterTableOperation::AddColumn { column_def, .. } => !approved(column_def),
                    AlterTableOperation::ChangeColumn { .. }
                    | AlterTableOperation::ModifyColumn { .. } => true,
                    AlterTableOperation::AlterColumn {
                        op: sqlparser::ast::AlterColumnOperation::SetDataType { .. },
                        ..
                    } => true,
                    _ => false,
                })
            }
            _ => false,
        };
        if invalid {
            ControlFlow::Break(
                "explicit column type or DDL implementation is not in the trusted subset",
            )
        } else {
            ControlFlow::Continue(())
        }
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        let allowed = match expr {
            Expr::Function(function) => {
                let current_timestamp = matches!(function.name.0.as_slice(),
                    [ObjectNamePart::Identifier(name)] if name.quote_style.is_none() && name.value.eq_ignore_ascii_case("current_timestamp"));
                ((self.engine == RdbmsEngine::Postgres && catalog_name(&function.name, "now"))
                    || (self.engine == RdbmsEngine::Mysql && current_timestamp)
                    || (self.engine == RdbmsEngine::Postgres
                        && current_timestamp
                        && matches!(function.args, FunctionArguments::None)))
                    && !function.uses_odbc_syntax
                    && matches!(function.parameters, FunctionArguments::None)
                    && (matches!(&function.args, FunctionArguments::List(args)
                    if args.args.is_empty() && args.clauses.is_empty() && args.duplicate_treatment.is_none())
                        || ((self.engine == RdbmsEngine::Mysql || current_timestamp)
                            && matches!(function.args, FunctionArguments::None)))
                    && function.filter.is_none()
                    && function.null_treatment.is_none()
                    && function.over.is_none()
                    && function.within_group.is_empty()
            }
            Expr::Cast {
                expr,
                data_type,
                array,
                format,
                ..
            } => {
                self.engine == RdbmsEngine::Postgres
                    && text_cast_source(expr).is_ok()
                    && matches!(data_type, DataType::Custom(name, modifiers)
                    if (catalog_name(name, "regclass") || catalog_name(name, "uuid")) && modifiers.is_empty())
                    && !array
                    && format.is_none()
            }
            Expr::BinaryOp { op, .. }
            | Expr::AnyOp { compare_op: op, .. }
            | Expr::AllOp { compare_op: op, .. } => matches!(
                op,
                BinaryOperator::Plus
                    | BinaryOperator::Minus
                    | BinaryOperator::Multiply
                    | BinaryOperator::Divide
                    | BinaryOperator::Modulo
                    | BinaryOperator::StringConcat
                    | BinaryOperator::Eq
                    | BinaryOperator::NotEq
                    | BinaryOperator::Gt
                    | BinaryOperator::GtEq
                    | BinaryOperator::Lt
                    | BinaryOperator::LtEq
                    | BinaryOperator::And
                    | BinaryOperator::Or
            ),
            Expr::UnaryOp { op, .. } => matches!(
                op,
                UnaryOperator::Plus | UnaryOperator::Minus | UnaryOperator::Not
            ),
            Expr::Identifier(_)
            | Expr::CompoundIdentifier(_)
            | Expr::Value(_)
            | Expr::Nested(_)
            | Expr::IsFalse(_)
            | Expr::IsNotFalse(_)
            | Expr::IsTrue(_)
            | Expr::IsNotTrue(_)
            | Expr::IsNull(_)
            | Expr::IsNotNull(_)
            | Expr::IsUnknown(_)
            | Expr::IsNotUnknown(_)
            | Expr::IsDistinctFrom(_, _)
            | Expr::IsNotDistinctFrom(_, _)
            | Expr::InList { .. }
            | Expr::InSubquery { .. }
            | Expr::Between { .. }
            | Expr::Like { .. }
            | Expr::ILike { .. }
            | Expr::Case { .. }
            | Expr::Exists { .. }
            | Expr::Subquery(_)
            | Expr::Tuple(_)
            | Expr::Wildcard(_)
            | Expr::QualifiedWildcard(_, _) => true,
            _ => false,
        };
        if allowed {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(
                "explicit server-side expression dispatch is not in the trusted subset",
            )
        }
    }
}

impl Visitor for SqlAstValidator {
    type Break = &'static str;

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<Self::Break> {
        if query.for_clause.is_some()
            || query.settings.is_some()
            || query.format_clause.is_some()
            || !query.pipe_operators.is_empty()
            || !supported_set_expr(&query.body)
        {
            ControlFlow::Break("unsupported or effectful query form")
        } else {
            ControlFlow::Continue(())
        }
    }

    fn pre_visit_select(&mut self, select: &Select) -> ControlFlow<Self::Break> {
        if select.into.is_some() || !select.lateral_views.is_empty()
            // A PostgreSQL type literal can otherwise be parsed as an expression
            // with a string alias, hiding its explicit type dispatch.
            || select.projection.iter().any(|item| matches!(item,
                sqlparser::ast::SelectItem::ExprWithAlias { alias, .. } if alias.quote_style == Some('\''))) {
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

fn supported_set_expr(expr: &SetExpr) -> bool {
    match expr {
        SetExpr::Select(_) | SetExpr::Values(_) => true,
        SetExpr::Query(query) => supported_set_expr(&query.body),
        SetExpr::SetOperation { left, right, .. } => {
            supported_set_expr(left) && supported_set_expr(right)
        }
        SetExpr::Insert(_) | SetExpr::Update(_) | SetExpr::Delete(_) | SetExpr::Merge(_) => true,
        SetExpr::Table(_) => false,
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
    if !values
        .iter()
        .any(|existing| existing == &value && existing.span() == value.span())
    {
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

fn object_name_parts(name: &ObjectName, engine: RdbmsEngine) -> Result<Vec<String>, TargetError> {
    let parts = name
        .0
        .iter()
        .map(|part| match part {
            ObjectNamePart::Identifier(identifier)
                if !identifier.value.is_empty() && identifier.value != "*" =>
            {
                // MySQL physical names depend on server settings and the filesystem;
                // unlike CTE aliases, they must not be folded unconditionally.
                Ok(if engine == RdbmsEngine::Mysql {
                    identifier.value.clone()
                } else {
                    sql_identifier(identifier, engine)
                })
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

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn rdbms_sql_targets(
        owner: EnvironmentOwnerPattern,
        engine: RdbmsEngine,
        database: &str,
        default_schema: &str,
        sql: &str,
    ) -> Result<Vec<PermissionTarget>, TargetError> {
        Ok(rdbms_sql_analysis(owner, engine, database, default_schema, sql)?.targets)
    }

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
            vec![
                ToolArgPattern::LongFlag {
                    name: ToolIdentifier("query".into()),
                    value: Some(ToolInvocationValue::Literal("two words".into())),
                },
                ToolArgPattern::Positional(ToolInvocationValue::Literal("literal *".into())),
            ],
            None,
        )
        .unwrap();

        assert!(matches!(
            target,
            PermissionTarget::Tool(ClassPermissionTarget {
                verb: Some(ToolVerb::Invoke),
                resource: ToolResourcePattern::Request(ToolInvocation {
                    command_path: path,
                    flags, positionals,
                }),
                ..
            }) if path == vec![ToolIdentifier("documents".into()), ToolIdentifier("find".into())]
                && flags == vec![
                    CanonicalToolFlag {
                        name: ToolIdentifier("query".into()),
                        value: Some(ToolInvocationValue::Literal("two words".into())),
                    },
                ]
                && positionals == vec![ToolInvocationValue::Literal("literal *".into())]
        ));

        assert!(
            tool_target(
                tool("root"),
                &[],
                vec![ToolArgPattern::Positional(ToolInvocationValue::Opaque)],
                None,
            )
            .is_ok()
        );
        assert!(tool_target(tool("bad"), &["not.valid"], vec![], None).is_err());
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

    fn ignite_tables(sql: &str) -> Vec<(RdbmsVerb, String, String, String)> {
        rdbms_sql_targets(env("prod"), RdbmsEngine::Ignite, "db", "PUBLIC", sql)
            .unwrap()
            .into_iter()
            .map(|target| match target {
                PermissionTarget::Rdbms(ClassPermissionTarget {
                    verb: Some(verb),
                    resource:
                        RdbmsResourcePattern::Table {
                            database,
                            schema,
                            table,
                        },
                    ..
                }) => (verb, database, schema, table),
                _ => panic!(),
            })
            .collect()
    }

    #[test]
    fn ignite_merge_shorthand_extracts_parameterized_and_quoted_destinations() {
        assert_eq!(
            ignite_tables("MERGE INTO ignite_counters (id, count) VALUES (?, 0)"),
            vec![(
                RdbmsVerb::Mutate,
                "db".to_string(),
                "PUBLIC".to_string(),
                "IGNITE_COUNTERS".to_string(),
            )]
        );
        assert_eq!(
            ignite_tables("MERGE INTO \"Counters\".\"Current\" (\"id\", \"count\") VALUES (?, ?)")
                [0],
            (
                RdbmsVerb::Mutate,
                "db".to_string(),
                "Counters".to_string(),
                "Current".to_string(),
            )
        );
    }

    #[test]
    fn ignite_merge_shorthand_extracts_select_sources() {
        assert_eq!(
            ignite_tables("MERGE INTO destination (id, count) SELECT id, count FROM source"),
            vec![
                (
                    RdbmsVerb::Mutate,
                    "db".to_string(),
                    "PUBLIC".to_string(),
                    "DESTINATION".to_string(),
                ),
                (
                    RdbmsVerb::Query,
                    "db".to_string(),
                    "PUBLIC".to_string(),
                    "SOURCE".to_string(),
                ),
            ]
        );
    }

    #[test]
    fn ignite_merge_shorthand_rejects_malformed_batches_and_key_clause() {
        for sql in [
            "MERGE INTO destination (id, count) VALUES (?, 0); SELECT * FROM protected",
            "MERGE INTO destination (id, count VALUES (?, 0)",
            "MERGE INTO destination (id, count) KEY (id) VALUES (?, 0)",
        ] {
            assert!(
                rdbms_sql_targets(env("prod"), RdbmsEngine::Ignite, "db", "PUBLIC", sql).is_err(),
                "accepted unsupported Ignite MERGE: {sql}"
            );
        }
    }

    #[test]
    fn ignite_merge_shorthand_does_not_broaden_other_dialects() {
        let sql = "MERGE INTO destination (id, count) VALUES (?, 0)";
        for engine in [RdbmsEngine::Postgres, RdbmsEngine::Mysql] {
            assert!(rdbms_sql_targets(env("prod"), engine, "db", "public", sql).is_err());
        }
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
                ("db", "AUDIT", "EVENTS"),
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
    fn sql_physical_names_use_dialect_spelling_without_folding_connection_identity() {
        use RdbmsEngine::{Ignite, Mysql, Postgres};
        for (engine, sql, database, schema, table) in [
            (
                Postgres,
                "SELECT * FROM ACCOUNTS",
                "Database",
                "DefaultSchema",
                "accounts",
            ),
            (
                Postgres,
                "SELECT * FROM Audit.Events",
                "Database",
                "audit",
                "events",
            ),
            (
                Postgres,
                "SELECT * FROM DB.Audit.Events",
                "db",
                "audit",
                "events",
            ),
            (
                Postgres,
                "SELECT * FROM \"DB\".\"Audit\".\"Events\"",
                "DB",
                "Audit",
                "Events",
            ),
            (
                Postgres,
                "SELECT * FROM ÄUDIT.ÉVENTS",
                "Database",
                "Äudit",
                "Évents",
            ),
            (
                Ignite,
                "SELECT * FROM accounts",
                "Database",
                "DefaultSchema",
                "ACCOUNTS",
            ),
            (
                Ignite,
                "SELECT * FROM audit.events",
                "Database",
                "AUDIT",
                "EVENTS",
            ),
            (
                Ignite,
                "SELECT * FROM db.audit.events",
                "DB",
                "AUDIT",
                "EVENTS",
            ),
            (
                Ignite,
                "SELECT * FROM \"db\".\"Audit\".\"events\"",
                "db",
                "Audit",
                "events",
            ),
            (
                Mysql,
                "SELECT * FROM Accounts",
                "Database",
                "DefaultSchema",
                "Accounts",
            ),
            (
                Mysql,
                "SELECT * FROM Tenant.Accounts",
                "Tenant",
                "Tenant",
                "Accounts",
            ),
            (
                Mysql,
                "SELECT * FROM `Tenant`.`Accounts`",
                "Tenant",
                "Tenant",
                "Accounts",
            ),
        ] {
            assert_eq!(
                rdbms_sql_targets(env("prod"), engine, "Database", "DefaultSchema", sql).unwrap(),
                vec![rdbms_target(env("prod"), RdbmsVerb::Query, database, schema, table).unwrap()],
                "{engine:?}: {sql}"
            );
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
            "EXPLAIN DROP TABLE protected",
            "SELECT * INTO copied FROM source",
            "SELECT * FROM dynamic_table(?)",
            "CREATE VIEW exposed AS SELECT * FROM protected",
            "DROP TABLE first,",
            "SELECT * FROM catalog.schema.table.extra",
            "WITH x AS (SELECT 1), x AS (SELECT * FROM protected) SELECT * FROM x",
            "WITH x AS (SELECT * FROM dynamic_table(?)) SELECT * FROM x",
            "SELECT * FROM accounts a FOR UPDATE OF accounts",
            "SELECT * FROM accounts a FOR UPDATE OF missing",
            "SELECT * FROM accounts a, protected a FOR UPDATE OF a",
            "SELECT * FROM (SELECT * FROM protected) a FOR UPDATE",
            "WITH x AS (SELECT * FROM protected) SELECT * FROM x FOR UPDATE OF x",
            "SELECT * FROM accounts FOR UPDATE OF public.accounts",
            "SELECT * FROM accounts UNION SELECT * FROM protected FOR UPDATE",
        ] {
            assert!(
                rdbms_sql_targets(env("prod"), RdbmsEngine::Postgres, "db", "public", sql,)
                    .is_err(),
                "accepted ambiguous SQL: {sql}"
            );
        }
    }

    fn assert_sql_tables(sql: &str, expected: &[(RdbmsVerb, &str)]) {
        let actual = tables(sql);
        assert_eq!(
            actual
                .iter()
                .map(|target| (target.0, target.3.as_str()))
                .collect::<Vec<_>>(),
            expected,
            "SQL: {sql}"
        );
    }

    #[test]
    fn sql_row_locks_require_mutate_only_on_locked_tables() {
        use RdbmsVerb::{Mutate, Query};
        for suffix in ["FOR UPDATE", "FOR SHARE NOWAIT", "FOR UPDATE SKIP LOCKED"] {
            assert_sql_tables(
                &format!("SELECT * FROM accounts a JOIN audit b ON true {suffix}"),
                &[
                    (Mutate, "accounts"),
                    (Mutate, "audit"),
                    (Query, "accounts"),
                    (Query, "audit"),
                ],
            );
        }
        assert_sql_tables(
            "SELECT * FROM accounts a JOIN audit b ON true WHERE EXISTS (SELECT 1 FROM protected) FOR UPDATE OF a",
            &[
                (Mutate, "accounts"),
                (Query, "accounts"),
                (Query, "audit"),
                (Query, "protected"),
            ],
        );
        assert_sql_tables(
            "SELECT * FROM accounts a, audit b FOR UPDATE OF a FOR SHARE OF b",
            &[
                (Mutate, "accounts"),
                (Mutate, "audit"),
                (Query, "accounts"),
                (Query, "audit"),
            ],
        );
        assert_sql_tables(
            "SELECT * FROM accounts AS \"A\" FOR UPDATE OF \"A\"",
            &[(Mutate, "accounts"), (Query, "accounts")],
        );
        assert_sql_tables(
            "SELECT * FROM accounts a WHERE EXISTS (SELECT 1 FROM protected a FOR SHARE OF a)",
            &[
                (Mutate, "protected"),
                (Query, "accounts"),
                (Query, "protected"),
            ],
        );
    }

    #[test]
    fn sql_indexes_and_table_locks_target_tables_not_index_names() {
        use RdbmsVerb::Mutate;
        assert_sql_tables(
            "CREATE INDEX idx ON protected(id)",
            &[(Mutate, "protected")],
        );
        assert_sql_tables(
            "CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx ON protected(id) INCLUDE (value) WHERE active = true",
            &[(Mutate, "protected")],
        );
        assert_sql_tables(
            "LOCK TABLE ONLY accounts, audit.events IN ACCESS SHARE MODE NOWAIT",
            &[(Mutate, "accounts"), (Mutate, "events")],
        );
    }

    #[test]
    fn sql_cte_aliases_are_scoped_and_never_hide_physical_qualified_tables() {
        use RdbmsVerb::Query;
        assert_sql_tables(
            "WITH visible AS (SELECT * FROM source), next AS (SELECT * FROM visible) SELECT * FROM next JOIN public.visible ON true",
            &[(Query, "source"), (Query, "visible")],
        );
        assert_sql_tables(
            "WITH visible AS (SELECT * FROM visible) SELECT * FROM visible",
            &[(Query, "visible")],
        );
        assert_sql_tables(
            "WITH first AS (SELECT * FROM later), later AS (SELECT * FROM protected) SELECT * FROM first",
            &[(Query, "later"), (Query, "protected")],
        );
        assert_sql_tables(
            "SELECT * FROM (WITH hidden AS (SELECT * FROM source) SELECT * FROM hidden) d JOIN hidden ON true",
            &[(Query, "source"), (Query, "hidden")],
        );
        assert_sql_tables(
            "WITH x AS (SELECT * FROM source) SELECT * FROM (WITH x AS (SELECT * FROM x) SELECT * FROM x) d",
            &[(Query, "source")],
        );
        assert_sql_tables(
            "WITH RECURSIVE tree AS (SELECT * FROM source UNION ALL SELECT s.* FROM source s JOIN tree t ON true) SELECT * FROM tree",
            &[(Query, "source")],
        );
        assert_sql_tables(
            "WITH \"Visible\" AS (SELECT * FROM source) SELECT * FROM \"Visible\" JOIN visible ON true",
            &[(Query, "source"), (Query, "visible")],
        );
    }

    #[test]
    fn sql_cte_identifier_equivalence_depends_on_the_engine() {
        use RdbmsEngine::{Ignite, Mysql, Postgres};
        for (engine, sql, physical) in [
            (
                Ignite,
                "WITH \"visible\" AS (SELECT 1) SELECT * FROM visible",
                Some("VISIBLE"),
            ),
            (
                Ignite,
                "WITH \"VISIBLE\" AS (SELECT 1) SELECT * FROM visible",
                None,
            ),
            (
                Ignite,
                "WITH visible AS (SELECT 1) SELECT * FROM \"visible\"",
                Some("visible"),
            ),
            (
                Ignite,
                "WITH visible AS (SELECT 1) SELECT * FROM \"VISIBLE\"",
                None,
            ),
            (
                Postgres,
                "WITH \"visible\" AS (SELECT 1) SELECT * FROM VISIBLE",
                None,
            ),
            (
                Postgres,
                "WITH \"VISIBLE\" AS (SELECT 1) SELECT * FROM visible",
                Some("visible"),
            ),
            (
                Postgres,
                "WITH \"ä\" AS (SELECT 1) SELECT * FROM Ä",
                Some("Ä"),
            ),
            (
                Postgres,
                "WITH Ä AS (SELECT 1) SELECT * FROM \"ä\"",
                Some("ä"),
            ),
            (Postgres, "WITH Ä AS (SELECT 1) SELECT * FROM \"Ä\"", None),
            (
                Mysql,
                "WITH `VISIBLE` AS (SELECT 1) SELECT * FROM visible",
                None,
            ),
            (
                Mysql,
                "WITH visible AS (SELECT 1) SELECT * FROM `VISIBLE`",
                None,
            ),
        ] {
            let expected = physical
                .map(|table| {
                    rdbms_target(env("prod"), RdbmsVerb::Query, "db", "public", table).unwrap()
                })
                .into_iter()
                .collect::<Vec<_>>();
            assert_eq!(
                rdbms_sql_targets(env("prod"), engine, "db", "public", sql).unwrap(),
                expected,
                "{engine:?}: {sql}"
            );
        }
        for (engine, sql, duplicate) in [
            (
                Ignite,
                "WITH \"visible\" AS (SELECT 1), visible AS (SELECT 2) SELECT 1",
                false,
            ),
            (
                Ignite,
                "WITH \"VISIBLE\" AS (SELECT 1), visible AS (SELECT 2) SELECT 1",
                true,
            ),
            (
                Postgres,
                "WITH \"VISIBLE\" AS (SELECT 1), visible AS (SELECT 2) SELECT 1",
                false,
            ),
            (
                Postgres,
                "WITH \"visible\" AS (SELECT 1), VISIBLE AS (SELECT 2) SELECT 1",
                true,
            ),
            (
                Mysql,
                "WITH `VISIBLE` AS (SELECT 1), visible AS (SELECT 2) SELECT 1",
                true,
            ),
        ] {
            assert_eq!(
                rdbms_sql_targets(env("prod"), engine, "db", "public", sql).is_err(),
                duplicate,
                "{engine:?}: {sql}"
            );
        }
    }

    #[test]
    fn sql_row_lock_alias_equivalence_depends_on_the_engine() {
        use RdbmsEngine::{Ignite, Mysql, Postgres};
        use RdbmsVerb::{Mutate, Query};
        for (engine, sql, locked) in [
            (
                Ignite,
                "SELECT * FROM accounts AS \"a\", audit AS \"A\" FOR UPDATE OF a",
                "audit",
            ),
            (
                Postgres,
                "SELECT * FROM accounts AS \"a\", audit AS \"A\" FOR UPDATE OF A",
                "accounts",
            ),
            (
                Postgres,
                "SELECT * FROM accounts AS \"Ä\", audit AS \"ä\" FOR UPDATE OF Ä",
                "accounts",
            ),
            (
                Mysql,
                "SELECT * FROM accounts AS `a`, audit AS `A` FOR UPDATE OF A",
                "audit",
            ),
        ] {
            let expected = [(Mutate, locked), (Query, "accounts"), (Query, "audit")]
                .into_iter()
                .map(|(verb, table)| {
                    let table = if engine == Ignite {
                        table.to_uppercase()
                    } else {
                        table.to_string()
                    };
                    rdbms_target(env("prod"), verb, "db", "public", &table).unwrap()
                })
                .collect::<Vec<_>>();
            assert_eq!(
                rdbms_sql_targets(env("prod"), engine, "db", "public", sql).unwrap(),
                expected,
                "{engine:?}: {sql}"
            );
        }
        for (engine, sql) in [
            (Ignite, "SELECT * FROM accounts AS \"a\" FOR UPDATE OF a"),
            (Postgres, "SELECT * FROM accounts AS \"A\" FOR UPDATE OF a"),
            (Mysql, "SELECT * FROM accounts AS `A` FOR UPDATE OF a"),
        ] {
            assert!(
                rdbms_sql_targets(env("prod"), engine, "db", "public", sql).is_err(),
                "{engine:?}: {sql}"
            );
        }
    }

    #[test]
    fn mysql_joined_updates_require_concrete_mutations_and_nested_reads() {
        use RdbmsVerb::{Mutate, Query};
        for (sql, reads) in [
            (
                "UPDATE accounts a JOIN audit b ON a.id=b.id SET a.value=b.value",
                vec![],
            ),
            (
                "UPDATE accounts a JOIN audit b ON a.id=b.id AND EXISTS (SELECT 1 FROM join_source) SET a.value=(SELECT value FROM value_source) WHERE a.id IN (SELECT id FROM filter_source)",
                vec!["join_source", "value_source", "filter_source"],
            ),
            (
                "UPDATE accounts a JOIN audit b ON a.id=b.id SET b.value=(SELECT value FROM accounts)",
                vec!["accounts"],
            ),
        ] {
            let expected = [(Mutate, "accounts"), (Mutate, "audit")]
                .into_iter()
                .chain(reads.into_iter().map(|table| (Query, table)))
                .map(|(verb, table)| rdbms_target(env("prod"), verb, "db", "db", table).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(
                rdbms_sql_targets(env("prod"), RdbmsEngine::Mysql, "db", "db", sql).unwrap(),
                expected,
                "SQL: {sql}"
            );
        }
        let sql = "UPDATE accounts a JOIN (SELECT * FROM audit) b ON a.id=b.id SET a.value=b.value";
        assert!(rdbms_sql_targets(env("prod"), RdbmsEngine::Mysql, "db", "db", sql).is_err());
    }

    #[test]
    fn sql_cte_writes_and_nested_locks_keep_their_own_verbs() {
        use RdbmsVerb::{Mutate, Query};
        assert_sql_tables(
            "WITH moved AS (DELETE FROM source RETURNING *) SELECT * FROM moved",
            &[(Mutate, "source")],
        );
        assert_sql_tables(
            "WITH moved AS (DELETE FROM source RETURNING *), saved AS (INSERT INTO destination SELECT * FROM moved RETURNING *) SELECT * FROM saved JOIN audit ON true",
            &[
                (Mutate, "source"),
                (Mutate, "destination"),
                (Query, "audit"),
            ],
        );
        assert_sql_tables(
            "WITH changed AS (UPDATE destination SET value = (SELECT value FROM protected) RETURNING *) SELECT * FROM changed",
            &[(Mutate, "destination"), (Query, "protected")],
        );
        assert_sql_tables(
            "WITH changed AS (DELETE FROM source RETURNING *) INSERT INTO destination SELECT * FROM changed",
            &[(Mutate, "source"), (Mutate, "destination")],
        );
        assert_sql_tables(
            "WITH visible AS (SELECT * FROM protected) SELECT * FROM visible, accounts FOR UPDATE",
            &[
                (Mutate, "accounts"),
                (Query, "protected"),
                (Query, "accounts"),
            ],
        );
        assert_sql_tables(
            "WITH visible AS (SELECT * FROM protected FOR SHARE) SELECT * FROM visible",
            &[(Mutate, "protected"), (Query, "protected")],
        );
        assert_sql_tables(
            "WITH moved AS (DELETE FROM source RETURNING *) SELECT * FROM source",
            &[(Mutate, "source"), (Query, "source")],
        );
    }

    #[test]
    fn sql_partial_index_conflict_predicates_preserve_sources_and_update_predicates() {
        use RdbmsVerb::{Mutate, Query};
        assert_sql_tables(
            "INSERT INTO accounts(email) VALUES ('a@example.com') ON CONFLICT (email) WHERE email IS NOT NULL DO NOTHING",
            &[(Mutate, "accounts")],
        );
        assert_sql_tables(
            "INSERT INTO accounts(id) VALUES (1) ON /* target */ CONFLICT (id) WHERE (active AND label = 'DO NOTHING') DO NOTHING",
            &[(Mutate, "accounts")],
        );
        assert_sql_tables(
            "INSERT INTO accounts(id) SELECT id FROM source WHERE true ON CONFLICT (id) WHERE active DO UPDATE SET id = (SELECT id FROM protected) WHERE EXISTS (SELECT 1 FROM audit)",
            &[
                (Mutate, "accounts"),
                (Query, "source"),
                (Query, "protected"),
                (Query, "audit"),
            ],
        );
        assert_sql_tables(
            "WITH changed AS (INSERT INTO accounts(id) VALUES (1) ON CONFLICT (id) WHERE active DO NOTHING RETURNING *) INSERT INTO destination SELECT * FROM changed ON CONFLICT (id) WHERE enabled DO NOTHING",
            &[(Mutate, "accounts"), (Mutate, "destination")],
        );
    }

    #[test]
    fn sql_explicit_expression_dispatch_requires_unanalyzed_authority() {
        for sql in [
            "SELECT read_secret()",
            "SELECT public.write_secret()",
            "SELECT pg_catalog.set_config('search_path', 'private', false)",
            "SELECT pg_catalog.now(1)",
            "SELECT pg_catalog.lower('A')",
            "SELECT (SELECT read_secret()) FROM allowed",
            "WITH nested AS (SELECT write_secret()) SELECT * FROM nested",
            "UPDATE allowed SET id = write_secret()",
            "CREATE TABLE allowed (id INT DEFAULT write_secret())",
            "CREATE TABLE allowed (id public.custom_type)",
            "CREATE TABLE allowed (id \"text\")",
            "CREATE TABLE allowed (id TIMESTAMPTZ)",
            "ALTER TABLE allowed ADD COLUMN id public.custom_type",
            "ALTER TABLE allowed ALTER COLUMN id TYPE public.custom_type",
            "SELECT 'x'::public.custom_type",
            "SELECT CAST('x' AS public.custom_type)",
            "SELECT public.custom_type 'x'",
            "SELECT CAST('1' AS integer)",
            "SELECT allowed.id::pg_catalog.regclass FROM allowed",
            "SELECT 1 OPERATOR(public.+) 2",
            "SELECT 1 OPERATOR(pg_catalog.+) 2",
            "SELECT 1 #@# 2",
            "SELECT id FROM allowed ORDER BY id USING OPERATOR(public.<)",
            "INSERT INTO allowed(id) VALUES (1) ON CONFLICT (id) WHERE write_secret() DO NOTHING",
            "INSERT INTO allowed(id) VALUES (1) ON CONFLICT (id) WHERE id = 'x'::public.custom_type DO NOTHING",
            "INSERT INTO allowed(id) VALUES (1) ON CONFLICT (id) WHERE id OPERATOR(public.=) 1 DO NOTHING",
        ] {
            assert!(
                matches!(
                    rdbms_sql_targets(env("prod"), RdbmsEngine::Postgres, "db", "public", sql),
                    Err(TargetError::SqlNotStaticallyExtractable(_))
                ),
                "accepted untrusted dispatch: {sql}"
            );
        }
        for engine in [RdbmsEngine::Mysql, RdbmsEngine::Ignite] {
            for sql in ["SELECT pg_catalog.now()", "SELECT pg_catalog.read_secret()"] {
                assert!(rdbms_sql_targets(env("prod"), engine, "db", "public", sql).is_err());
            }
        }
    }

    #[test]
    fn sql_default_schema_qualifies_only_physical_relations() {
        for (sql, expected) in [
            (
                "WITH x AS (SELECT * FROM accounts) SELECT x.id, a.id FROM x JOIN accounts a ON x.id = a.id",
                "WITH x AS (SELECT * FROM public.accounts) SELECT x.id, a.id FROM x JOIN public.accounts a ON x.id = a.id",
            ),
            (
                "WITH accounts AS (SELECT * FROM accounts) SELECT * FROM accounts",
                "WITH accounts AS (SELECT * FROM public.accounts) SELECT * FROM accounts",
            ),
            (
                "WITH RECURSIVE x AS (SELECT id FROM accounts UNION ALL SELECT id FROM x) SELECT * FROM x",
                "WITH RECURSIVE x AS (SELECT id FROM public.accounts UNION ALL SELECT id FROM x) SELECT * FROM x",
            ),
            (
                "SELECT 'accounts', a.id FROM /* keep */ \"Accounts\" a FOR UPDATE OF a",
                "SELECT 'accounts', a.id FROM /* keep */ public.\"Accounts\" a FOR UPDATE OF a",
            ),
            (
                "CREATE TABLE child (a INTEGER REFERENCES parent(id), b INTEGER REFERENCES parent(id), FOREIGN KEY(a) REFERENCES parent(id))",
                "CREATE TABLE public.child (a INTEGER REFERENCES public.parent(id), b INTEGER REFERENCES public.parent(id), FOREIGN KEY(a) REFERENCES public.parent(id))",
            ),
            (
                "CREATE INDEX idx ON accounts(id)",
                "CREATE INDEX idx ON public.accounts(id)",
            ),
            (
                "ALTER TABLE accounts RENAME TO renamed",
                "ALTER TABLE public.accounts RENAME TO renamed",
            ),
            (
                "ALTER TABLE tenant.accounts RENAME TO renamed",
                "ALTER TABLE tenant.accounts RENAME TO renamed",
            ),
            (
                "DROP TABLE accounts, tenant.notes",
                "DROP TABLE public.accounts, tenant.notes",
            ),
            (
                "TRUNCATE accounts, notes",
                "TRUNCATE public.accounts, public.notes",
            ),
            (
                "LOCK TABLE accounts, notes",
                "LOCK TABLE public.accounts, public.notes",
            ),
            ("SELECT * FROM \"only\"", "SELECT * FROM public.\"only\""),
            (
                "UPDATE public.only SET id = 1",
                "UPDATE public.only SET id = 1",
            ),
            (
                "CREATE TABLE child (LIKE parent)",
                "CREATE TABLE public.child (LIKE public.parent)",
            ),
            (
                "SELECT 'é',\n a.id FROM accounts a",
                "SELECT 'é',\n a.id FROM public.accounts a",
            ),
        ] {
            assert_eq!(
                rdbms_sql_execution(RdbmsEngine::Postgres, sql, |_| false).unwrap(),
                expected,
                "{sql}"
            );
        }
        for sql in [
            "SELECT * FROM ONLY accounts",
            "UPDATE ONLY accounts SET id = 1",
            "DELETE FROM ONLY accounts",
        ] {
            assert!(matches!(
                rdbms_sql_targets(env("prod"), RdbmsEngine::Postgres, "db", "public", sql),
                Err(TargetError::SqlNotStaticallyExtractable(_))
            ));
            assert!(matches!(
                rdbms_sql_execution(RdbmsEngine::Postgres, sql, |_| false),
                Err(TargetError::SqlNotStaticallyExtractable(_))
            ));
        }
        let targets = rdbms_sql_targets(
            env("prod"),
            RdbmsEngine::Postgres,
            "db",
            "public",
            "ALTER TABLE tenant.accounts RENAME TO renamed",
        )
        .unwrap();
        assert_eq!(
            targets,
            vec![
                rdbms_target(env("prod"), RdbmsVerb::Mutate, "db", "tenant", "accounts").unwrap(),
                rdbms_target(env("prod"), RdbmsVerb::Mutate, "db", "tenant", "renamed").unwrap(),
            ]
        );
        assert!(
            rdbms_sql_execution(
                RdbmsEngine::Postgres,
                "CREATE TEMP TABLE accounts(id INTEGER)",
                |_| false
            )
            .is_err()
        );
    }

    #[test]
    fn sql_regclass_lookups_keep_parameters_and_explicit_names() {
        for (sql, source) in [
            ("SELECT $1::regclass", "$1"),
            ("SELECT CAST($1 AS pg_catalog.regclass)", "$1"),
        ] {
            let expected_operand = format!(
                "(CASE WHEN pg_catalog.regexp_match({source}, E'^[ \\t\\n\\r\\013\\014]*(?:\"(?:[^\"]|\"\")*\"[^.]*|[^\". \\t\\n\\r\\013\\014][^.]*)$') IS NOT NULL THEN pg_catalog.format('public.%s', {source}) ELSE {source} END)"
            );
            let expected = if sql.contains("CAST(") {
                format!("SELECT CAST({expected_operand} AS pg_catalog.regclass)")
            } else {
                format!("SELECT {expected_operand}::pg_catalog.regclass")
            };
            assert_eq!(
                rdbms_sql_execution(RdbmsEngine::Postgres, sql, |index| index == 1).unwrap(),
                expected
            );
        }
        for (sql, expected) in [
            (
                "SELECT 'accounts'::regclass",
                "SELECT E'public.accounts'::pg_catalog.regclass",
            ),
            (
                "SELECT 'migration-log'::regclass",
                "SELECT E'public.migration-log'::pg_catalog.regclass",
            ),
            (
                "SELECT 'o\"brien'::regclass",
                "SELECT E'public.o\"brien'::pg_catalog.regclass",
            ),
            (
                "SELECT '\u{2003}\"abc.def\"'::regclass",
                "SELECT '\u{2003}\"abc.def\"'::pg_catalog.regclass",
            ),
            (
                "SELECT '\t\n\r\u{b}\u{c} \"abc.def\"'::regclass",
                "SELECT E'public.\t\n\r\u{b}\u{c} \"abc.def\"'::pg_catalog.regclass",
            ),
            (
                "SELECT 'tenant.accounts'::regclass",
                "SELECT 'tenant.accounts'::pg_catalog.regclass",
            ),
            (
                "SELECT '\"Accounts\"'::regclass",
                "SELECT E'public.\"Accounts\"'::pg_catalog.regclass",
            ),
            (
                "SELECT $$\"a.b\"$$::regclass",
                "SELECT E'public.\"a.b\"'::pg_catalog.regclass",
            ),
            (
                "SELECT E'accounts'::regclass",
                "SELECT E'public.accounts'::pg_catalog.regclass",
            ),
            (
                "SELECT '\"O''Brien\"'::regclass",
                "SELECT E'public.\"O''Brien\"'::pg_catalog.regclass",
            ),
        ] {
            assert_eq!(
                rdbms_sql_execution(RdbmsEngine::Postgres, sql, |_| false).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn sql_migrator_dispatch_is_qualified_without_regenerating_sql() {
        for (sql, executable) in [
            ("SELECT $1::uuid", "SELECT $1::pg_catalog.uuid"),
            (
                "SELECT CAST($1 AS pg_catalog.uuid)",
                "SELECT CAST($1 AS pg_catalog.uuid)",
            ),
            (
                "SELECT 'now() ::regclass', /* now() */ now ( /* keep */ ) -- now()\n",
                "SELECT 'now() ::regclass', /* now() */ pg_catalog.now ( /* keep */ ) -- now()\n",
            ),
            (
                "SELECT 'é',\r\n NOW(), $1 :: /* type */ \"uuid\"",
                "SELECT 'é',\r\n pg_catalog.NOW(), $1 :: /* type */ pg_catalog.\"uuid\"",
            ),
            (
                "CREATE TABLE migrations (id INTEGER PRIMARY KEY, name TEXT, created_at TIMESTAMP NOT NULL DEFAULT now())",
                "CREATE TABLE public.migrations (id INTEGER PRIMARY KEY, name pg_catalog.TEXT, created_at TIMESTAMP NOT NULL DEFAULT pg_catalog.now())",
            ),
            (
                "ALTER TABLE migrations ADD COLUMN stamp DATE",
                "ALTER TABLE public.migrations ADD COLUMN stamp pg_catalog.DATE",
            ),
            ("SELECT now FROM now", "SELECT now FROM public.now"),
            ("CREATE TABLE now ()", "CREATE TABLE public.now ()"),
            (
                "INSERT INTO allowed(id) VALUES (1) ON CONFLICT (id) WHERE now() IS NOT NULL DO NOTHING",
                "INSERT INTO public.allowed(id) VALUES (1) ON CONFLICT (id) WHERE pg_catalog.now() IS NOT NULL DO NOTHING",
            ),
        ] {
            let analysis =
                rdbms_sql_analysis(env("prod"), RdbmsEngine::Postgres, "db", "public", sql)
                    .unwrap();
            assert_eq!(
                rdbms_sql_execution(RdbmsEngine::Postgres, sql, |_| true).unwrap(),
                executable
            );
            assert_eq!(
                analysis.targets,
                rdbms_sql_targets(env("prod"), RdbmsEngine::Postgres, "db", "public", sql).unwrap()
            );
            assert_eq!(
                rdbms_sql_execution(RdbmsEngine::Postgres, executable, |_| true).unwrap(),
                executable
            );
        }
        for sql in [
            "SELECT public.now()",
            "SELECT \"NOW\"()",
            "SELECT $1::public.regclass",
        ] {
            assert!(rdbms_sql_execution(RdbmsEngine::Postgres, sql, |_| true).is_err());
        }
    }

    #[test]
    fn sql_ordinary_rdbms_fixtures_use_only_trusted_dispatch() {
        for (sql, executable, parameters) in [
            (
                "CREATE TABLE IF NOT EXISTS users (user_id uuid NOT NULL PRIMARY KEY, name text NOT NULL, tags text[], created_on timestamp DEFAULT NOW());",
                "CREATE TABLE IF NOT EXISTS public.users (user_id pg_catalog.uuid NOT NULL PRIMARY KEY, name pg_catalog.text NOT NULL, tags pg_catalog.text[], created_on timestamp DEFAULT pg_catalog.NOW());",
                vec![],
            ),
            (
                "CREATE TABLE migration (id INTEGER, created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP)",
                "CREATE TABLE public.migration (id INTEGER, created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP)",
                vec![],
            ),
            (
                "INSERT INTO users (user_id, name, tags) VALUES ($1::uuid, $2, $3)",
                "INSERT INTO public.users (user_id, name, tags) VALUES ($1::pg_catalog.uuid, $2, $3)",
                vec![1],
            ),
        ] {
            let analysis =
                rdbms_sql_analysis(env("prod"), RdbmsEngine::Postgres, "db", "public", sql)
                    .unwrap();
            assert_eq!(
                rdbms_sql_execution(RdbmsEngine::Postgres, sql, |index| index == 1).unwrap(),
                executable
            );
            assert_eq!(analysis.text_cast_parameters, parameters);
            assert!(analysis.validate_parameters(|index| index == 1).is_ok());
            assert!(
                rdbms_sql_targets(env("prod"), RdbmsEngine::Postgres, "db", "public", sql).is_ok()
            );
        }
        for sql in [
            "SELECT \"CURRENT_TIMESTAMP\"()",
            "SELECT public.current_timestamp()",
            "SELECT CURRENT_TIMESTAMP OVER ()",
            "SELECT CURRENT_TIMESTAMP()",
        ] {
            assert!(
                rdbms_sql_execution(RdbmsEngine::Postgres, sql, |_| false).is_err(),
                "{sql}"
            );
        }
        let sql = "CREATE TABLE IF NOT EXISTS users (user_id varchar(25) NOT NULL, name varchar(255) NOT NULL, created_on timestamp NOT NULL DEFAULT NOW(), PRIMARY KEY (user_id));";
        assert_eq!(
            rdbms_sql_execution(RdbmsEngine::Mysql, sql, |_| false).unwrap(),
            sql.replace("NOW()", "CURRENT_TIMESTAMP()")
        );
        assert!(rdbms_sql_targets(env("prod"), RdbmsEngine::Mysql, "db", "db", sql).is_ok());
        for sql in [
            "SELECT CURRENT_TIMESTAMP",
            "SELECT CURRENT_TIMESTAMP()",
            "SELECT now ( /* keep */ )",
        ] {
            assert!(
                rdbms_sql_execution(RdbmsEngine::Mysql, sql, |_| false).is_ok(),
                "{sql}"
            );
        }
        for sql in [
            "SELECT `now`()",
            "SELECT public.now()",
            "SELECT `CURRENT_TIMESTAMP`()",
            "SELECT other_function()",
            "SELECT now(1)",
        ] {
            assert!(
                matches!(
                    rdbms_sql_targets(env("prod"), RdbmsEngine::Mysql, "db", "db", sql),
                    Err(TargetError::SqlNotStaticallyExtractable(_))
                ),
                "{sql}"
            );
        }
        for sql in [
            "CREATE TABLE users (id public.uuid)",
            "CREATE TABLE users (tags public.text[])",
            "SELECT $1::public.uuid",
        ] {
            assert!(
                rdbms_sql_targets(env("prod"), RdbmsEngine::Postgres, "db", "public", sql).is_err(),
                "{sql}"
            );
        }
    }

    #[test]
    fn sql_trusted_casts_require_text_sources_and_actual_parameter_types() {
        for sql in [
            "SELECT $2::regclass, $1::uuid, $2::pg_catalog.regclass",
            "SELECT (SELECT $1::uuid) FROM allowed WHERE id = $2::regclass",
            "INSERT INTO allowed(id) VALUES (1) ON CONFLICT (id) WHERE $2::regclass = $1::regclass DO NOTHING",
        ] {
            let analysis =
                rdbms_sql_analysis(env("prod"), RdbmsEngine::Postgres, "db", "public", sql)
                    .unwrap();
            assert_eq!(analysis.text_cast_parameters, vec![1, 2]);
            assert!(analysis.validate_parameters(|index| index <= 2).is_ok());
            for text_indices in [vec![], vec![1], vec![2]] {
                assert!(
                    matches!(
                        analysis.validate_parameters(|index| text_indices.contains(&index)),
                        Err(TargetError::SqlNotStaticallyExtractable(_))
                    ),
                    "{sql}"
                );
                assert!(
                    rdbms_sql_execution(RdbmsEngine::Postgres, sql, |index| text_indices
                        .contains(&index))
                    .is_err()
                );
            }
        }
        for sql in [
            "SELECT 'allowed'::regclass",
            "SELECT E'allowed'::regclass",
            "SELECT $$allowed$$::regclass",
            "SELECT '00000000-0000-0000-0000-000000000000'::uuid",
        ] {
            let analysis =
                rdbms_sql_analysis(env("prod"), RdbmsEngine::Postgres, "db", "public", sql)
                    .unwrap();
            assert!(analysis.text_cast_parameters.is_empty());
            assert!(analysis.validate_parameters(|_| false).is_ok());
        }
        for sql in [
            "SELECT 1::regclass",
            "SELECT NULL::regclass",
            "SELECT true::regclass",
            "SELECT B'1'::regclass",
            "SELECT id::regclass FROM allowed",
            "SELECT ($1::public.payload)::regclass",
            "SELECT $0::uuid",
            "SELECT $999999999999999999999999::regclass",
        ] {
            assert!(
                matches!(
                    rdbms_sql_targets(env("prod"), RdbmsEngine::Postgres, "db", "public", sql),
                    Err(TargetError::SqlNotStaticallyExtractable(_))
                ),
                "{sql}"
            );
        }
    }

    #[test]
    fn sql_explicit_index_dispatch_requires_unanalyzed_authority() {
        for sql in [
            "CREATE INDEX allowed_idx ON public.allowed (id public.evil_ops)",
            "CREATE INDEX allowed_idx ON public.allowed USING evil_am (id)",
            "CREATE INDEX allowed_idx ON public.allowed (id pg_catalog.int4_ops)",
            "CREATE INDEX allowed_idx ON public.allowed USING btree (id)",
        ] {
            assert!(
                Parser::parse_sql(&PostgreSqlDialect {}, sql).is_ok(),
                "{sql}"
            );
            assert!(
                matches!(
                    rdbms_sql_targets(env("prod"), RdbmsEngine::Postgres, "db", "public", sql),
                    Err(TargetError::SqlNotStaticallyExtractable(_))
                ),
                "{sql}"
            );
        }
        assert!(
            rdbms_sql_targets(
                env("prod"),
                RdbmsEngine::Postgres,
                "db",
                "public",
                "CREATE INDEX allowed_idx ON public.allowed (id)"
            )
            .is_ok()
        );
    }

    #[test]
    fn sql_ddl_implementation_selectors_require_unanalyzed_authority() {
        for sql in [
            "CREATE TABLE allowed (id INTEGER) USING evil_table_am",
            "CREATE TABLE allowed (id INTEGER) USING heap",
            "CREATE TABLE allowed (id INTEGER, PRIMARY KEY (id public.evil_ops))",
            "CREATE TABLE allowed (id INTEGER, UNIQUE (id public.evil_ops))",
            "ALTER TABLE allowed ADD CONSTRAINT key PRIMARY KEY (id public.evil_ops)",
            "ALTER TABLE allowed ADD CONSTRAINT key UNIQUE USING INDEX evil_index",
            "CREATE TABLE allowed (id INTEGER) WITH (implementation = 'evil')",
            "CREATE TABLE allowed (name TEXT COLLATE public.evil_collation)",
            "ALTER TABLE allowed SET ACCESS METHOD evil_table_am",
            "ALTER TABLE allowed ALTER COLUMN name SET COMPRESSION evil_compression",
        ] {
            assert!(
                matches!(
                    rdbms_sql_targets(env("prod"), RdbmsEngine::Postgres, "db", "public", sql),
                    Err(TargetError::SqlNotStaticallyExtractable(_))
                ),
                "{sql}"
            );
        }
        for sql in [
            "CREATE TABLE allowed (id INTEGER) ENGINE = evil_engine",
            "CREATE TABLE allowed (id INTEGER, INDEX custom_idx USING evil_am (id))",
            "CREATE TABLE allowed (id INTEGER, UNIQUE custom_idx USING evil_am (id))",
            "ALTER TABLE allowed ADD INDEX custom_idx USING evil_am (id)",
        ] {
            assert!(Parser::parse_sql(&MySqlDialect {}, sql).is_ok(), "{sql}");
            assert!(
                matches!(
                    rdbms_sql_targets(env("prod"), RdbmsEngine::Mysql, "db", "db", sql),
                    Err(TargetError::SqlNotStaticallyExtractable(_))
                ),
                "{sql}"
            );
        }
        for sql in [
            "CREATE TABLE allowed (id INTEGER PRIMARY KEY)",
            "CREATE TABLE allowed (id INTEGER, UNIQUE (id))",
            "ALTER TABLE allowed ADD CONSTRAINT key UNIQUE (id)",
        ] {
            assert!(
                rdbms_sql_targets(env("prod"), RdbmsEngine::Postgres, "db", "public", sql).is_ok(),
                "{sql}"
            );
        }
    }

    #[test]
    fn sql_partial_index_conflict_predicates_reject_subqueries_and_malformed_sql() {
        for sql in [
            "INSERT INTO accounts(id) VALUES (1) ON CONFLICT (id) WHERE EXISTS (SELECT 1 FROM protected) DO NOTHING",
            "INSERT INTO accounts(id) VALUES (1) ON CONFLICT (id) WHERE id IN (SELECT id FROM protected) DO NOTHING",
            "INSERT INTO accounts(id) VALUES (1) ON CONFLICT (id) WHERE (SELECT true) DO NOTHING",
            "INSERT INTO accounts(id) VALUES (1) ON CONFLICT (id) WHERE active = DO NOTHING",
            "INSERT INTO accounts(id) VALUES (1) ON CONFLICT (id) WHERE DO NOTHING",
            "INSERT INTO accounts(id) VALUES (1) ON CONFLICT (id) WHERE active NOTHING",
            "INSERT INTO accounts(id) VALUES (1) ON CONFLICT (id) WHERE active DO UPDATE SET",
            "INSERT INTO accounts(id) VALUES (1) ON CONFLICT (id) WHERE active DO NOTHING; DELETE FROM protected",
            "INSERT INTO accounts(id) VALUES (1) ON CONFLICT (id) WHERE active; DELETE FROM protected; DO NOTHING",
        ] {
            assert!(
                rdbms_sql_targets(env("prod"), RdbmsEngine::Postgres, "db", "public", sql).is_err(),
                "accepted unsupported conflict predicate: {sql}",
            );
        }
        for engine in [RdbmsEngine::Mysql, RdbmsEngine::Ignite] {
            assert!(
                rdbms_sql_targets(
                    env("prod"),
                    engine,
                    "db",
                    "public",
                    "INSERT INTO accounts(id) VALUES (1) ON CONFLICT (id) WHERE active DO NOTHING",
                )
                .is_err()
            );
        }
    }
}
