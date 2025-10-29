use std::fmt;
use std::fmt::Display;
use serde::{de, Deserialize, Deserializer, Serialize};
use url::Url;

#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Author {
    pub login: String
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct Id(pub u64);

impl Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where D: Deserializer<'de>
    {
        struct IdVisitor;
        impl<'de> de::Visitor<'de> for IdVisitor {
            type Value = Id;
            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
                where E: de::Error {
                Ok(Id(value))
            }
            fn visit_str<E>(self, id: &str) -> Result<Self::Value, E>
                where E: de::Error {
                id.parse::<u64>().map(Id).map_err(de::Error::custom)
            }
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "expected  id as number or string")
            }
        }

        deserializer.deserialize_any(IdVisitor)
    }
 }

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Repository {
    pub id: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<Author>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fork: Option<bool>,
    pub url: Url,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignees_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blobs_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branches_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collaborators_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compare_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contents_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contributors_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployments_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloads_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forks_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_commits_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_refs_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_tags_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_comment_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_events_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issues_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keys_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub languages_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merges_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub milestones_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notifications_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pulls_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub releases_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stargazers_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statuses_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribers_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub teams_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trees_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clone_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirror_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hooks_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub svn_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<::serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forks_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stargazers_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watchers_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_issues_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_template: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topics: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_issues: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_projects: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_wiki: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_pages: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_downloads: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    #[serde( skip_serializing_if = "Option::is_none")]
    pub pushed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_rebase_merge: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_repository: Option<Box<Repository>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_squash_merge: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_merge_commit: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_update_branch: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_forking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribers_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_auto_merge: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete_branch_on_merge: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Box<Repository>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Box<Repository>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Notification {
    pub id: Id,
    pub repository: Repository,
    pub subject: Subject,
    pub reason: String,
    pub unread: bool,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub last_read_at: Option<chrono::DateTime<chrono::Utc>>,
    pub url: Url,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Reason {
    Assign,
    Author,
    Comment,
    Invitation,
    Manual,
    Mention,
    #[serde(rename = "review_requested")]
    ReviewRequested,
    #[serde(rename = "security_alert")]
    SecurityAlert,
    #[serde(rename = "state_change")]
    StateChange,
    Subscribed,
    #[serde(rename = "team_mention")]
    TeamMention,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Subject {
    pub title: String,
    pub url: Option<Url>,
    pub latest_comment_url: Option<Url>,
    pub r#type: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ThreadSubscription {
    pub subscribed: bool,
    pub ignored: bool,
    pub reason: Option<Reason>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub url: Url,
    pub thread_url: Url,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Issue {
    pub id: Id,
    pub node_id: String,
    pub url: Url,
    pub repository_url: Url,
    pub labels_url: Url,
    pub comments_url: Url,
    pub events_url: Url,
    pub html_url: Url,
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_html: Option<String>,
    pub user: Author,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<Author>,
    pub assignees: Vec<Author>,
    pub author_association: String,
    pub locked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_lock_reason: Option<String>,
    pub comments: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_by: Option<Author>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct IssueComment {
    pub id: Id,
    pub node_id: String,
    pub url: Url,
    pub html_url: Url,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_html: Option<String>,
    pub user: Author,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PullRequest {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_comments_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_comment_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statuses_url: Option<Url>,
    pub number: u64,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub maintainer_can_modify: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<Box<Author>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_lock_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mergeable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_by: Option<Box<Author>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_commit_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<Box<Author>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignees: Option<Vec<Author>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_reviewers: Option<Vec<Author>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebaseable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<Box<Repository>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additions: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletions: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_files: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_comments: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PullRequestComment {
    pub id: Id,
    pub node_id: String,
    pub url: Url,
    pub html_url: Url,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_html: Option<String>,
    pub user: Author,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}
