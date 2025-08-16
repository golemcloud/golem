mod bindings;
mod github;

use crate::bindings::exports::pa_ck::na_me_exports::component_name_api::*;
use crate::bindings::golem::llm::llm::{self, ChatEvent, CompleteResponse, ContentPart};
use std::collections::HashSet;
use std::sync::{LazyLock, RwLock};
use chrono::{Utc, DateTime};
use reqwest::{self, header};
use serde::de::DeserializeOwned;
use serde::Deserialize;

/// The internal status of our component. This will be automatically persisted by golem.
struct State {
    last_checked: Option<DateTime<Utc>>,
    pending_notifications: Vec<EnhancedNotification>,
    seen_notifications: HashSet<String>,
    github_username: String,
    github_auth_token: String,
    client: reqwest::Client
}

static STATE: LazyLock<RwLock<Option<State>>> = LazyLock::new(|| RwLock::new(None));

const GITHUB_API_HOST: &str = "https://api.github.com";

static LLM_CONFIG: LazyLock<llm::Config> = LazyLock::new(|| llm::Config {
    model: "gpt-4.1-nano".to_string(),
    temperature: Some(0.2),
    tools: vec![],
    provider_options: vec![],
    max_tokens: None,
    tool_choice: None,
    stop_sequences: None
});

const LLM_RESPONSE_FORMAT: &str = r#"
{
    "category": string, // one of action-needed, fyi, releases, ci-cd
    "priority": string, // one of urgent, high, normal, low
    "summary": string, // summary of the notification and what it is about
    "action-items": array<string>, // list of actions that the recipient of the notification needs to perform
    "key-points": array<string> // most important parts of the notification
}
"#;

struct Component;

impl Guest for Component {
    fn initialize(args: InitializeArgs) {
        let mut state = STATE.write().unwrap();
        let _ = state.insert(State {
            last_checked: None,
            pending_notifications: Vec::new(),
            seen_notifications: HashSet::new(),
            github_username: args.github_username,
            github_auth_token: args.github_auth_token,
            client: reqwest::Client::builder().user_agent("golem-ai-agent").build().unwrap()
        });
    }

    fn get_last_checked() -> Option<String> {
        let guard = STATE.read().unwrap();
        let state = guard.as_ref().unwrap();

        state.last_checked.map(|lc| lc.to_rfc3339())
    }

    fn get_whats_new() -> WhatsNewResult {
        let mut guard = STATE.write().unwrap();
        let state = guard.as_mut().unwrap();

        let now = update_notifications(state);

        let mut result = WhatsNewResult {
            action_needed: Vec::new(),
            fyi: Vec::new(),
            releases: Vec::new(),
            ci_cd: Vec::new(),
            last_checked: now.to_rfc3339()
        };

        for notification in state.pending_notifications.iter() {
            match notification.category {
                Category::ActionNeeded => result.action_needed.push(notification.clone()),
                Category::CiCd => result.ci_cd.push(notification.clone()),
                Category::Fyi => result.fyi.push(notification.clone()),
                Category::Releases => result.fyi.push(notification.clone()),
            }
        };

        result
    }

    fn has_seen_notification(id: NotificationId) -> bool {
        let guard = STATE.read().unwrap();
        let state = guard.as_ref().unwrap();

        state.seen_notifications.contains(&id)
    }

    fn mark_all_seen() {
        let mut guard = STATE.write().unwrap();
        let state = guard.as_mut().unwrap();

        for notification in state.pending_notifications.iter() {
            state.seen_notifications.insert(notification.id.clone());
        }
        state.pending_notifications.clear();
    }

    fn mark_notification_seen(id: NotificationId) {
        let mut guard = STATE.write().unwrap();
        let state = guard.as_mut().unwrap();

        state.seen_notifications.insert(id.clone());

        let index = state.pending_notifications.iter().position(|pn| pn.id == id);
        if let Some(index) = index {
            state.pending_notifications.swap_remove(index);
        }
    }
}

fn update_notifications(state: &mut State) -> DateTime<Utc> {
    let since = state.last_checked.as_ref();
    let before = chrono::offset::Utc::now();

    let new_notifications = get_notifications(state, since, &before);

    for notification in new_notifications {
        match notification.subject.r#type.as_str() {
            "Issue" => state.pending_notifications.push(process_issue_notification(state, since, &before, notification)),
            "PullRequest" => state.pending_notifications.push(process_pull_request_notification(state, since, &before, notification)),
            "CheckSuite" => state.pending_notifications.push(process_check_suite_notification(notification)),
            other => {
                // Optionally add support for other notification type heer
                println!("Ignoring notification for subject type {other}")
            }
        }
    }

    state.last_checked = Some(before);
    return before
}

fn process_pull_request_notification(
    state: &State,
    since: Option<&DateTime<Utc>>,
    before: &DateTime<Utc>,
    notification: github::Notification
) -> EnhancedNotification {
    let pull_request = get_notification_subject::<github::PullRequest>(state, &notification);

    if pull_request.comments_url.is_none() {
        panic!("body: {}", serde_json::to_string(&notification).unwrap());
    };

    let pull_request_comments = get_pull_request_comments(state, &pull_request, since, &before);
    let llm_response = llm::send(
        &vec![
            llm::Message {
                role: llm::Role::System,
                name: None,
                content: vec![
                    llm::ContentPart::Text(
                        format!(
                            r#"
                                You are used as part of an application to automatically categorize GitHub notifications.
                                Your responses should have the following json structure:

                                ```
                                {LLM_RESPONSE_FORMAT}
                                ```
                                Your entire response should be valid json. Do not include any headers / trailers like ``` that would prevent
                                the response from being parsed as json.

                                The GitHub handle of the user reading the notifications is `{}`

                                The notification is for a pull request, this is the raw notification:
                                ```
                                {}
                                ```

                                This is the pull request. Pay special attention to whether the author of the pull
                                request is the current user and tailor your response accordingly:
                                ```
                                {}
                                ```

                                And these are the new comments in the pull request:
                                ```
                                {}
                                ```

                                Make a response summarizing what happened in the pull request as part of the new comments in the requested format
                            "#,
                            state.github_username,
                            serde_json::to_string(&notification).unwrap(),
                            serde_json::to_string(&pull_request).unwrap(),
                            serde_json::to_string(&pull_request_comments).unwrap(),
                        )
                    )
                ]
            }
        ],
        &LLM_CONFIG
    );

    let parsed_llm_response = parse_llm_response(llm_response);

    EnhancedNotification {
        id: notification.id.to_string(),
        reason: notification.reason,
        subject: Subject {
            type_: SubjectType::PullRequest,
            title: notification.subject.title,
            url: notification.subject.url.map(|u| u.to_string())
        },
        repository: Repository {
            owner: notification.repository.owner.unwrap().login,
            name: notification.repository.name
        },
        action_items: parsed_llm_response.action_items,
        priority: parsed_llm_response.priority.into(),
        updated_at: notification.updated_at.to_rfc3339(),
        url: notification.url.to_string(),
        category: parsed_llm_response.category.into(),
        summary: parsed_llm_response.summary,
        key_points: parsed_llm_response.key_points
    }
}

fn process_issue_notification(
    state: &State,
    since: Option<&DateTime<Utc>>,
    before: &DateTime<Utc>,
    notification: github::Notification
) -> EnhancedNotification {
    let issue = get_notification_subject::<github::Issue>(state, &notification);
    let issue_comments = get_issue_comments(state, &issue, since, &before);
    let llm_response = llm::send(
        &vec![
            llm::Message {
                role: llm::Role::System,
                name: None,
                content: vec![
                    llm::ContentPart::Text(
                        format!(
                            r#"
                                You are used as part of an application to automatically categorize GitHub notifications.
                                Your responses should have the following json structure:

                                ```
                                {LLM_RESPONSE_FORMAT}
                                ```
                                Your entire response should be valid json. Do not include any headers / trailers like ``` that would prevent
                                the response from being parsed as json.

                                The GitHub handle of the user reading the notifications is `{}`

                                The notification is for an issue, this is the raw notification:
                                ```
                                {}
                                ```

                                This is the issue. Pay special attention to whether the author of the issue
                                is the current user and tailor your response accordingly:
                                ```
                                {}
                                ```

                                And these are the new comments in the issue:
                                ```
                                {}
                                ```

                                Make a response summarizing what happened in the issue as part of the new comments in the requested format
                            "#,
                            state.github_username,
                            serde_json::to_string(&notification).unwrap(),
                            serde_json::to_string(&issue).unwrap(),
                            serde_json::to_string(&issue_comments).unwrap(),
                        )
                    )
                ]
            }
        ],
        &LLM_CONFIG
    );

    let parsed_llm_response = parse_llm_response(llm_response);

    EnhancedNotification {
        id: notification.id.to_string(),
        reason: notification.reason,
        subject: Subject {
            type_: SubjectType::Issue,
            title: notification.subject.title,
            url: notification.subject.url.map(|u| u.to_string())
        },
        repository: Repository {
            owner: notification.repository.owner.unwrap().login,
            name: notification.repository.name
        },
        action_items: parsed_llm_response.action_items,
        priority: parsed_llm_response.priority.into(),
        updated_at: notification.updated_at.to_rfc3339(),
        url: notification.url.to_string(),
        category: parsed_llm_response.category.into(),
        summary: parsed_llm_response.summary,
        key_points: parsed_llm_response.key_points
    }
}

fn process_check_suite_notification(
    notification: github::Notification
) -> EnhancedNotification {
    EnhancedNotification {
        id: notification.id.to_string(),
        reason: notification.reason,
        subject: Subject {
            type_: SubjectType::CheckSuite,
            title: notification.subject.title.clone(),
            url: notification.subject.url.map(|u| u.to_string())
        },
        repository: Repository {
            owner: notification.repository.owner.unwrap().login,
            name: notification.repository.name
        },
        action_items: vec![],
        priority: Priority::Normal,
        updated_at: notification.updated_at.to_rfc3339(),
        url: notification.url.to_string(),
        category: Category::CiCd,
        summary: notification.subject.title,
        key_points: vec![]
    }
}

fn get_notifications(state: &State, since: Option<&DateTime<Utc>>, before: &DateTime<Utc>) -> Vec<github::Notification> {
    // Add support for pagination here to handle users with a lot of notifications
    let mut query_params = vec![("before", before.to_rfc3339())];
    if let Some(since) = since {
        query_params.push(("since", since.to_rfc3339()));
    }

    let res = state.client
        .get(format!("{}/notifications", GITHUB_API_HOST))
        .query(&query_params)
        .header(header::AUTHORIZATION, format!("Bearer {}", state.github_auth_token))
        .send()
        .unwrap();

    res.json().unwrap()
}

fn get_notification_subject<T: DeserializeOwned>(state: &State, notification: &github::Notification) -> T {
    let res = state.client
        .get(notification.subject.url.clone().unwrap())
        .header(header::AUTHORIZATION, format!("Bearer {}", state.github_auth_token))
        .send()
        .unwrap();

    res.json().unwrap()
}

fn get_issue_comments(state: &State, issue: &github::Issue, since: Option<&DateTime<Utc>>, before: &DateTime<Utc>) -> Vec<github::IssueComment> {
    // Add pagination here to support issues with a lot of comments
    let mut query_params = vec![("before", before.to_rfc3339())];
    if let Some(since) = since {
        query_params.push(("since", since.to_rfc3339()));
    }

    let res = state.client
        .get(issue.comments_url.clone())
        .query(&query_params)
        .header(header::AUTHORIZATION, format!("Bearer {}", state.github_auth_token))
        .send()
        .unwrap();

    res.json().unwrap()
}

fn get_pull_request_comments(state: &State, pull_request: &github::PullRequest, since: Option<&DateTime<Utc>>, before: &DateTime<Utc>) -> Vec<github::PullRequestComment> {
    // Add pagination here to support pull requests with a lot of comments
    let mut query_params = vec![("before", before.to_rfc3339())];
    if let Some(since) = since {
        query_params.push(("since", since.to_rfc3339()));
    }

    let res = state.client
        .get(pull_request.comments_url.clone().unwrap())
        .query(&query_params)
        .header(header::AUTHORIZATION, format!("Bearer {}", state.github_auth_token))
        .send()
        .unwrap();

    res.json().unwrap()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all="kebab-case")]
pub enum LLMResponseCategory {
    ActionNeeded,
    Fyi,
    Releases,
    CiCd,
}

impl From<LLMResponseCategory> for Category {
    fn from(value: LLMResponseCategory) -> Self {
        match value {
            LLMResponseCategory::ActionNeeded => Category::ActionNeeded,
            LLMResponseCategory::Fyi => Category::Fyi,
            LLMResponseCategory::Releases => Category::Releases,
            LLMResponseCategory::CiCd => Category::CiCd
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all="kebab-case")]
pub enum LLMResponsePriority {
    Urgent,
    High,
    Normal,
    Low,
}

impl From<LLMResponsePriority> for Priority {
    fn from(value: LLMResponsePriority) -> Self {
        match value {
            LLMResponsePriority::High => Priority::High,
            LLMResponsePriority::Low => Priority::Low,
            LLMResponsePriority::Normal => Priority::Normal,
            LLMResponsePriority::Urgent => Priority::Urgent
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all="kebab-case")]
struct LLMResponse {
    category: LLMResponseCategory,
    priority: LLMResponsePriority,
    summary: String,
    action_items: Vec<String>,
    key_points: Vec<String>
}

fn parse_llm_response(raw_response: ChatEvent) -> LLMResponse {
    match raw_response {
        ChatEvent::Message(CompleteResponse {
            content, ..
        }) => {
            match content.as_slice() {
                [ContentPart::Text(text)] => {
                    serde_json::from_str(&text).unwrap()
                }
                _ => panic!("received unexpected content from llm")
            }
        }
        _ => panic!("received unexpected response from llm")
    }
}

bindings::export!(Component with_types_in bindings);
