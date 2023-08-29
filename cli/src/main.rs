extern crate derive_more;

use std::fmt::{Debug, Display, Formatter};
use clap::{ArgMatches, Error, FromArgMatches, Parser, Subcommand};
use std::path::PathBuf;
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use uuid::Uuid;
use clap_verbosity_flag::Verbosity;
use derive_more::{Display, FromStr};
use chrono::{Utc, DateTime};
use tokio;
use serde::{Serialize, Deserialize};

#[derive(Clone, PartialEq, Eq, Debug)]
struct ProjectId(Uuid);

#[derive(Clone, PartialEq, Eq, Debug)]
enum ProjectRef {
    Id(ProjectId),
    Name(String),
    Default,
}



impl FromArgMatches for ProjectRef {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        ProjectRefArgs::from_arg_matches(matches).map(|c| (&c).into())
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let prc0: ProjectRefArgs = (&self.clone()).into();
        let mut prc = prc0.clone();
        let res = ProjectRefArgs::update_from_arg_matches(&mut prc, matches);
        *self = (&prc).into();
        res
    }
}

impl clap::Args for ProjectRef {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        ProjectRefArgs::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        ProjectRefArgs::augment_args_for_update(cmd)
    }
}

#[derive(clap::Args, Debug, Clone)]
struct ProjectRefArgs {
    #[arg(short = 'P', long, conflicts_with = "project_name")]
    project_id: Option<Uuid>,

    #[arg(short = 'p', long, conflicts_with = "project_id")]
    project_name: Option<String>,
}

impl From<&ProjectRefArgs> for ProjectRef {
    fn from(value: &ProjectRefArgs) -> ProjectRef {
        if let Some(id) = value.project_id {
            ProjectRef::Id(ProjectId(id))
        } else if let Some(name) = value.project_name.clone() {
            ProjectRef::Name(name)
        } else {
            ProjectRef::Default
        }
    }
}

impl From<&ProjectRef> for ProjectRefArgs {
    fn from(value: &ProjectRef) -> Self {
        match value {
            ProjectRef::Id(ProjectId(id)) => {
                ProjectRefArgs { project_id: Some(id.clone()), project_name: None }
            }
            ProjectRef::Name(name) => {
                ProjectRefArgs { project_id: None, project_name: Some(name.clone()) }
            }
            ProjectRef::Default => {
                ProjectRefArgs { project_id: None, project_name: None }
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct RawComponentId(Uuid);

#[derive(Clone, PartialEq, Eq, Debug)]
enum ComponentIdOrName {
    Id(RawComponentId),
    Name(ComponentName, ProjectRef),
}

impl FromArgMatches for ComponentIdOrName {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        ComponentIdOrNameArgs::from_arg_matches(matches).map(|c| (&c).into())
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let prc0: ComponentIdOrNameArgs = (&self.clone()).into();
        let mut prc = prc0.clone();
        let res = ComponentIdOrNameArgs::update_from_arg_matches(&mut prc, matches);
        *self = (&prc).into();
        res
    }
}

impl clap::Args for ComponentIdOrName {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        ComponentIdOrNameArgs::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        ComponentIdOrNameArgs::augment_args_for_update(cmd)
    }
}

#[derive(clap::Args, Debug, Clone)]
struct ComponentIdOrNameArgs {
    #[arg(short = 'C', long, conflicts_with = "component_name", required = true)]
    component_id: Option<Uuid>,

    #[arg(short, long, conflicts_with = "component_id", required = true)]
    component_name: Option<String>,

    #[arg(short = 'P', long, conflicts_with = "project_name", conflicts_with = "component_id")]
    project_id: Option<Uuid>,

    #[arg(short = 'p', long, conflicts_with = "project_id", conflicts_with = "component_id")]
    project_name: Option<String>,
}


impl From<&ComponentIdOrNameArgs> for ComponentIdOrName {
    fn from(value: &ComponentIdOrNameArgs) -> ComponentIdOrName {
        let pr = if let Some(id) = value.project_id {
            ProjectRef::Id(ProjectId(id))
        } else if let Some(name) = value.project_name.clone() {
            ProjectRef::Name(name)
        } else {
            ProjectRef::Default
        };

        if let Some(id) = value.component_id {
            ComponentIdOrName::Id(RawComponentId(id))
        } else {
            ComponentIdOrName::Name(ComponentName(value.component_name.as_ref().unwrap().to_string()), pr)
        }
    }
}

impl From<&ComponentIdOrName> for ComponentIdOrNameArgs {
    fn from(value: &ComponentIdOrName) -> ComponentIdOrNameArgs {
        match value {
            ComponentIdOrName::Id(RawComponentId(id)) => {
                ComponentIdOrNameArgs { component_id: Some(id.clone()), component_name: None, project_id: None, project_name: None }
            }
            ComponentIdOrName::Name(ComponentName(name), pr) => {
                let (project_id, project_name) = match pr {
                    ProjectRef::Id(ProjectId(id)) => {
                        (Some(*id), None)
                    }
                    ProjectRef::Name(name) => {
                        (None, Some(name.to_string()))
                    }
                    ProjectRef::Default => {
                        (None, None)
                    }
                };

                ComponentIdOrNameArgs { component_id: None, component_name: Some(name.clone()), project_id, project_name }
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
struct ComponentName(String); // TODO: Validate

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
struct InstanceName(String); // TODO: Validate

fn parse_key_val(s: &str) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{s}`"))?;
    Ok((s[..pos].parse()?, s[pos + 1..].parse()?))
}

fn parse_instant(s: &str) -> Result<DateTime<Utc>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    match s.parse::<DateTime<Utc>>() {
        Ok(dt) => Ok(dt),
        Err(err) => Err(err.into())
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
struct AccountId(String); // TODO: Validate

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
struct ProjectPolicyId(Uuid);

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
struct TemplateName(String);

#[derive(Subcommand, Debug)]
#[command()]
enum Command {
    #[command()]
    Deploy {
        #[command(flatten)]
        project_ref: ProjectRef,

        #[arg(short, long)]
        component_name: ComponentName,

        #[arg(short, long)]
        instance_name: InstanceName,

        #[arg(short, long, value_parser = parse_key_val)]
        env: Vec<(String, String)>,

        #[arg(short, long)]
        function: String,

        #[arg(short = 'j', long)]
        parameters: String, // TODO: validate json

        #[arg(value_name = "component-file", value_hint = clap::ValueHint::FilePath)]
        component_file: PathBuf, // TODO: validate exists,

        #[arg(value_name = "args")]
        args: Vec<String>,
    },
    #[command()]
    Component {
        #[command(subcommand)]
        subcommand: ComponentSubcommand
    },
    #[command()]
    Instance {
        #[command(subcommand)]
        subcommand: InstanceSubcommand
    },
    #[command()]
    Account {
        #[arg(short = 'A', long)]
        account_id: Option<AccountId>,

        #[command(subcommand)]
        subcommand: AccountSubcommand,
    },
    #[command()]
    Token {
        #[arg(short = 'A', long)]
        account_id: Option<AccountId>,

        #[command(subcommand)]
        subcommand: TokenSubcommand,
    },
    #[command()]
    Project {
        #[command(subcommand)]
        subcommand: ProjectSubcommand,
    },
    #[command()]
    Share {
        #[command(flatten)]
        project_ref: ProjectRef,

        #[arg(long)]
        recipient_account_id: AccountId,

        #[arg(long, conflicts_with = "project_actions")]
        project_policy_id: ProjectPolicyId,

        #[arg(short = 'A', long, conflicts_with = "project_policy_id")]
        project_actions: Vec<ProjectAction>,
    },
    #[command()]
    ProjectPolicy {
        #[command(subcommand)]
        subcommand: ProjectPolicySubcommand
    },
    #[command()]
    New {
        #[arg(short, long)]
        template: TemplateName,

        #[arg(short, long)]
        component_name: ComponentName,

        #[arg(short, long)]
        package_name: Option<String>,
    },
    #[command()]
    ListTemplates {
        #[arg(short, long)]
        min_tier: Option<GuestLanguageTier>,

        #[arg(short, long)]
        language: Option<GuestLanguage>,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter)]
enum ProjectAction {
    ViewComponent,
    CreateComponent,
    UpdateComponent,
    DeleteComponent,
    ViewInstance,
    CreateInstance,
    UpdateInstance,
    DeleteInstance,
    ViewProjectGrants,
    CreateProjectGrants,
    DeleteProjectGrants,
}

impl Display for ProjectAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ProjectAction::ViewComponent => "ViewComponent",
            ProjectAction::CreateComponent => "CreateComponent",
            ProjectAction::UpdateComponent => "UpdateComponent",
            ProjectAction::DeleteComponent => "DeleteComponent",
            ProjectAction::ViewInstance => "ViewInstance",
            ProjectAction::CreateInstance => "CreateInstance",
            ProjectAction::UpdateInstance => "UpdateInstance",
            ProjectAction::DeleteInstance => "DeleteInstance",
            ProjectAction::ViewProjectGrants => "ViewProjectGrants",
            ProjectAction::CreateProjectGrants => "CreateProjectGrants",
            ProjectAction::DeleteProjectGrants => "DeleteProjectGrants",
        };

        Display::fmt(s, f)
    }
}

impl FromStr for ProjectAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ViewComponent" => Ok(ProjectAction::ViewComponent),
            "CreateComponent" => Ok(ProjectAction::CreateComponent),
            "UpdateComponent" => Ok(ProjectAction::UpdateComponent),
            "DeleteComponent" => Ok(ProjectAction::DeleteComponent),
            "ViewInstance" => Ok(ProjectAction::ViewInstance),
            "CreateInstance" => Ok(ProjectAction::CreateInstance),
            "UpdateInstance" => Ok(ProjectAction::UpdateInstance),
            "DeleteInstance" => Ok(ProjectAction::DeleteInstance),
            "ViewProjectGrants" => Ok(ProjectAction::ViewProjectGrants),
            "CreateProjectGrants" => Ok(ProjectAction::CreateProjectGrants),
            "DeleteProjectGrants" => Ok(ProjectAction::DeleteProjectGrants),
            _ => {
                let all =
                    ProjectAction::iter()
                        .map(|x| format!("\"{x}\""))
                        .collect::<Vec<String>>()
                        .join(", ");
                Err(format!("Unknown action: {s}. Expected one of {all}"))
            }
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter)]
enum GuestLanguageTier {
    Tier1,
    Tier2,
    Tier3,
    Tier4,
}

impl Display for GuestLanguageTier {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            GuestLanguageTier::Tier1 => "1",
            GuestLanguageTier::Tier2 => "2",
            GuestLanguageTier::Tier3 => "3",
            GuestLanguageTier::Tier4 => "4",
        };

        Display::fmt(s, f)
    }
}

impl FromStr for GuestLanguageTier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_ref() {
            "tier1" | "1" => Ok(GuestLanguageTier::Tier1),
            "tier2" | "2" => Ok(GuestLanguageTier::Tier2),
            "tier3" | "3" => Ok(GuestLanguageTier::Tier3),
            "tier4" | "4" => Ok(GuestLanguageTier::Tier4),
            _ => Err(format!("Unexpected guest language tier {s}")),
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter)]
enum GuestLanguage {
    Rust,
    Go,
    C,
    Zig,
    JavaScript,
    CSharp,
    Swift,
    Grain,
    Python,
}

impl Display for GuestLanguage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            GuestLanguage::Rust => "Rust",
            GuestLanguage::Go => "Go",
            GuestLanguage::C => "C",
            GuestLanguage::Zig => "Zig",
            GuestLanguage::JavaScript => "JavaScript",
            GuestLanguage::CSharp => "C#",
            GuestLanguage::Swift => "Swift",
            GuestLanguage::Grain => "Grain",
            GuestLanguage::Python => "Python",
        };

        Display::fmt(s, f)
    }
}

impl FromStr for GuestLanguage {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_ref() {
            "rust" => Ok(GuestLanguage::Rust),
            "go" => Ok(GuestLanguage::Go),
            "c" | "c++" | "cpp" => Ok(GuestLanguage::C),
            "zig" => Ok(GuestLanguage::Zig),
            "js" | "javascript" => Ok(GuestLanguage::JavaScript),
            "c#" | "cs" | "csharp" => Ok(GuestLanguage::CSharp),
            "swift" => Ok(GuestLanguage::Swift),
            "grain" => Ok(GuestLanguage::Grain),
            "py" | "python" => Ok(GuestLanguage::Python),
            _ => {
                let all =
                    GuestLanguage::iter()
                        .map(|x| format!("\"{x}\""))
                        .collect::<Vec<String>>()
                        .join(", ");
                Err(format!("Unknown language: {s}. Expected one of {all}"))
            },
        }
    }
}

#[derive(Subcommand, Debug)]
#[command()]
enum ComponentSubcommand {
    #[command()]
    Add {
        #[command(flatten)]
        project_ref: ProjectRef,

        #[arg(short, long)]
        component_name: ComponentName,

        #[arg(value_name = "component-file", value_hint = clap::ValueHint::FilePath)]
        component_file: PathBuf, // TODO: validate exists
    },

    #[command()]
    Update {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(value_name = "component-file", value_hint = clap::ValueHint::FilePath)]
        component_file: PathBuf, // TODO: validate exists
    },

    #[command()]
    List {
        #[command(flatten)]
        project_ref: ProjectRef,

        #[arg(short, long)]
        component_name: Option<ComponentName>,
    },
}


#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
struct InvocationKey(String); // TODO: Validate

#[derive(Subcommand, Debug)]
#[command()]
enum InstanceSubcommand {
    #[command()]
    Add {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,

        #[arg(short, long, value_parser = parse_key_val)]
        env: Vec<(String, String)>,

        #[arg(value_name = "args")]
        args: Vec<String>,
    },
    #[command()]
    InvocationKey {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
    #[command()]
    InvokeAndAwait {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,

        #[arg(short = 'k', long)]
        invocation_key: Option<InvocationKey>,

        #[arg(short, long)]
        function: String,

        #[arg(short = 'j', long)]
        parameters: String, // TODO: validate json

        #[arg(short = 's', long, default_value_t = false)]
        use_stdio: bool,
    },
    #[command()]
    Invoke {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,

        #[arg(short, long)]
        function: String,

        #[arg(short = 'j', long)]
        parameters: String, // TODO: validate json
    },
    #[command()]
    Connect {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
    #[command()]
    Interrupt {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
    #[command()]
    SimulatedCrash {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
    #[command()]
    Delete {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
    #[command()]
    Get {
        #[command(flatten)]
        component_id_or_name: ComponentIdOrName,

        #[arg(short, long)]
        instance_name: InstanceName,
    },
}

#[derive(Subcommand, Debug)]
#[command()]
enum AccountSubcommand {
    #[command()]
    Get {},

    #[command()]
    Update {
        // TODO: validate non-empty
        #[arg(short, long)]
        account_name: Option<String>,

        #[arg(short = 'e', long)]
        account_email: Option<String>,
    },

    #[command()]
    New {
        #[arg(short, long)]
        account_name: String,

        #[arg(short = 'e', long)]
        account_email: String,
    },

    #[command()]
    Delete {},

    #[command()]
    Grant {
        #[command(subcommand)]
        subcommand: GrantSubcommand,
    },

}

#[derive(Subcommand, Debug)]
#[command()]
enum GrantSubcommand {
    #[command()]
    Get {},

    #[command()]
    Add {
        #[arg(value_name = "ROLE")]
        role: Role
    },

    #[command()]
    Delete {
        #[arg(value_name = "ROLE")]
        role: Role
    },
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter)]
enum Role {
    Admin,
    WhitelistAdmin,
    MarketingAdmin,
    ViewProject,
    DeleteProject,
    CreateProject,
    InstanceServer,
}

impl Display for Role {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Role::Admin => { "Admin" }
            Role::WhitelistAdmin => { "WhitelistAdmin" }
            Role::MarketingAdmin => { "MarketingAdmin" }
            Role::ViewProject => { "ViewProject" }
            Role::DeleteProject => { "DeleteProject" }
            Role::CreateProject => { "CreateProject" }
            Role::InstanceServer => { "InstanceServer" }
        };

        Display::fmt(s, f)
    }
}

impl FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Admin" => Ok(Role::Admin),
            "WhitelistAdmin" => Ok(Role::WhitelistAdmin),
            "MarketingAdmin" => Ok(Role::MarketingAdmin),
            "ViewProject" => Ok(Role::ViewProject),
            "DeleteProject" => Ok(Role::DeleteProject),
            "CreateProject" => Ok(Role::CreateProject),
            "InstanceServer" => Ok(Role::InstanceServer),
            _ => {
                let all =
                    Role::iter()
                        .map(|x| format!("\"{x}\""))
                        .collect::<Vec<String>>()
                        .join(", ");
                Err(format!("Unknown role: {s}. Expected one of {all}"))
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
struct TokenId(Uuid);

#[derive(Subcommand, Debug)]
#[command()]
enum TokenSubcommand {
    #[command()]
    List {},

    #[command()]
    Add {
        #[arg(long, value_parser = parse_instant, default_value = "2100-01-01T00:00:00Z")]
        expires_at: DateTime<Utc>
    },

    #[command()]
    Delete {
        #[arg(value_name = "TOKEN")]
        token_id: TokenId
    },

}

#[derive(Subcommand, Debug)]
#[command()]
enum ProjectSubcommand {
    #[command()]
    Add {
        #[arg(short, long)]
        project_name: String,

        #[arg(short = 't', long)]
        project_description: Option<String>,
    },

    #[command()]
    List {
        #[arg(short, long)]
        project_name: Option<String>,
    },

    #[command()]
    GetDefault {},
}

#[derive(Subcommand, Debug)]
#[command()]
enum ProjectPolicySubcommand {
    #[command()]
    Add {
        #[arg(long)]
        project_policy_name: String,

        #[arg(value_name = "Actions")]
        project_actions: Vec<ProjectAction>,
    },

    #[command()]
    Get {
        #[arg(value_name = "ID")]
        project_policy_id: ProjectPolicyId,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter)]
enum Format {
    Json,
    Yaml,
}

impl Display for Format {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
        };
        Display::fmt(&s, f)
    }
}

impl FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "json" => Ok(Format::Json),
            "yaml" => Ok(Format::Yaml),
            _ => {
                let all =
                    Format::iter()
                        .map(|x| format!("\"{x}\""))
                        .collect::<Vec<String>>()
                        .join(", ");
                Err(format!("Unknown format: {s}. Expected one of {all}"))
            }
        }
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None, rename_all = "kebab-case")]
struct GolemCommand {
    #[arg(short = 'D', long, value_name = "DIR", value_hint = clap::ValueHint::DirPath)]
    config_directory: Option<PathBuf>,

    #[arg(short = 'T', long)]
    auth_token: Option<Uuid>, // TODO: uuid

    #[command(flatten)]
    verbosity: Verbosity,

    #[arg(short = 'F', long, default_value = "yaml")]
    format: Format,

    #[command(subcommand)]
    command: Command,
}

fn main() ->  Result<(), Box<dyn std::error::Error>> {
    let command = GolemCommand::parse();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main(command))
}

enum GolemResult {
    Ok(Box<dyn PrintRes>),
    Err(String)
}

#[derive(Debug, Serialize, Deserialize)]
struct DeployResult {
    msg: String
}


impl <T> PrintRes for T
where T: Serialize, {
    fn println(&self, format: &Format) -> () {
        match format {
            Format::Json => println!("{}", serde_json::to_string(self).unwrap()),
            Format::Yaml => println!("{}", serde_yaml::to_string(self).unwrap()),
        }
    }
}

trait PrintRes {
    fn println(&self, format: &Format) -> ();
}

async fn process_deploy(cmd: &Command) -> GolemResult {
    GolemResult::Ok(Box::new(DeployResult{ msg: format!("{:?}", cmd) }))
}

async fn process_component(cmd: &Command) -> GolemResult {
     GolemResult::Ok(Box::new(DeployResult{ msg: format!("{:?}", cmd) }))
}
async fn process_instance(cmd: &Command) -> GolemResult {
     GolemResult::Ok(Box::new(DeployResult{ msg: format!("{:?}", cmd) }))
}
async fn process_account(cmd: &Command) -> GolemResult {
     GolemResult::Ok(Box::new(DeployResult{ msg: format!("{:?}", cmd) }))
}
async fn process_token(cmd: &Command) -> GolemResult {
     GolemResult::Ok(Box::new(DeployResult{ msg: format!("{:?}", cmd) }))
}
async fn process_project(cmd: &Command) -> GolemResult {
     GolemResult::Ok(Box::new(DeployResult{ msg: format!("{:?}", cmd) }))
}
async fn process_share(cmd: &Command) -> GolemResult {
     GolemResult::Ok(Box::new(DeployResult{ msg: format!("{:?}", cmd) }))
}
async fn process_project_policy(cmd: &Command) -> GolemResult {
     GolemResult::Ok(Box::new(DeployResult{ msg: format!("{:?}", cmd) }))
}
async fn process_new(cmd: &Command) -> GolemResult {
     GolemResult::Ok(Box::new(DeployResult{ msg: format!("{:?}", cmd) }))
}
async fn process_list_templates(cmd: &Command) -> GolemResult {
     GolemResult::Ok(Box::new(DeployResult{ msg: format!("{:?}", cmd) }))
}

async fn async_main(cmd: GolemCommand) -> Result<(), Box<dyn std::error::Error>> {
    let res = match cmd.command {
        c@ Command::Deploy { .. } => process_deploy(&c).await,
        c@ Command::Component { .. } => process_component(&c).await,
        c@ Command::Instance { .. } => process_instance(&c).await,
        c@ Command::Account { .. } => process_account(&c).await,
        c@ Command::Token { .. } => process_token(&c).await,
        c@ Command::Project { .. } => process_project(&c).await,
        c@ Command::Share { .. } => process_share(&c).await,
        c@ Command::ProjectPolicy { .. } => process_project_policy(&c).await,
        c@ Command::New { .. } => process_new(&c).await,
        c@ Command::ListTemplates { .. } => process_list_templates(&c).await,
    };

    match res {
        GolemResult::Ok(r) => {
            r.println(&cmd.format);

            Ok(())
        }
        GolemResult::Err(_) => todo!(),
    }
}