# make sure this stays before other urllib uses
from wit_world import exports
from wit_world.exports.component_name_api import *
from wit_world.imports import llm
from typing import Optional, Set
from dataclasses import dataclass
import os
from urllib3 import request
from typing import Any
import schema
import json
from datetime import datetime, timezone


@dataclass
class State:
    last_checked: Optional[str]
    pending_notifications: list[EnhancedNotification]
    seen_notifications: Set[str]
    github_username: str
    github_auth_token: str


state: Optional[State] = None

GITHUB_API_HOST = "https://api.github.com"

llm_config = llm.Config(
    model="gpt-4.1-nano",
    temperature=0.2,
    tools=[],
    provider_options=[],
    max_tokens=None,
    tool_choice=None,
    stop_sequences=None,
)


def get_state() -> State:
    if state is None:
        raise ValueError("Worker is not initialized")
    return state


class ComponentNameApi(exports.ComponentNameApi):
    def initialize(self, args: InitializeArgs) -> None:
        # check that OPENAI_API_KEY is defined and nonempty, otherwise model api calls will fail.
        if not os.getenv("OPENAI_API_KEY"):
            raise ValueError("OPENAI_API_KEY env var is empty or not defined")

        global state
        state = State(
            last_checked=None,
            pending_notifications=[],
            seen_notifications=set(),
            github_username=args.github_username,
            github_auth_token=args.github_auth_token,
        )

    def get_last_checked(self) -> Optional[str]:
        return get_state().last_checked

    def get_whats_new(self) -> WhatsNewResult:
        last_checked = update_notifications()
        result = WhatsNewResult(
            action_needed=[], fyi=[], releases=[], ci_cd=[], last_checked=last_checked
        )
        for notification in get_state().pending_notifications:
            if notification.category == Category.ACTION_NEEDED:
                result.action_needed.append(notification)
            elif notification.category == Category.FYI:
                result.fyi.append(notification)
            elif notification.category == Category.RELEASES:
                result.releases.append(notification)
            else:
                result.ci_cd.append(notification)
        return result

    def has_seen_notification(self, id: str) -> bool:
        return id in get_state().seen_notifications

    def mark_all_seen(self) -> None:
        state = get_state()
        for notification in state.pending_notifications:
            state.seen_notifications.add(notification.id)
        state.pending_notifications.clear()

    def mark_notification_seen(self, id: str) -> None:
        state = get_state()
        state.seen_notifications.add(id)
        filtered_notifications = list(
            filter(lambda n: n.id != id, state.pending_notifications)
        )
        state.pending_notifications = filtered_notifications


def update_notifications() -> str:
    state = get_state()
    since = state.last_checked
    before = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

    new_notifications = get_notifications(since, before)

    results: list[EnhancedNotification] = []
    for notification in new_notifications:
        notification_subject = notification["subject"]["type"]
        if notification_subject == "Issue":
            results.append(process_issue_notification(since, before, notification))
        elif notification_subject == "PullRequest":
            results.append(process_issue_notification(since, before, notification))
        elif notification_subject == "CheckSuite":
            results.append(process_check_suite_notification(notification))
        else:
            # Optionally add support for other notification types here
            print(f"Ignoring notification for subject {notification_subject}")

    state.last_checked = before
    state.pending_notifications.extend(results)

    return before


def github_user() -> str:
    return get_state().github_username


def github_auth_token() -> str:
    return get_state().github_auth_token


def get_notifications(since: Optional[str], before: str) -> Any:
    # Add support for pagination here if you have a lot of notifications
    query = f"all=true&before={before}"
    if since is not None:
        query += f"&since={since}"

    res = request(
        "GET",
        f"https://api.github.com/notifications?{query}",
        headers={"authorization": f"Bearer {github_auth_token()}"},
    )
    return res.json()


def process_pull_request_notification(
    since: Optional[str], before: str, notification: Any
) -> EnhancedNotification:
    pull_request = get_notification_subject(notification)
    comments = get_notification_subject_comments(since, before, pull_request)
    llm_response = llm.send(
        [
            llm.Message(
                role=llm.Role.SYSTEM,
                name=None,
                content=[
                    llm.ContentPart_Text(
                        f"""
                                You are used as part of an application to automatically categorize GitHub notifications.
                                Your responses should have the following json structure:

                                ```
                                {model_response_format}
                                ```
                                Your entire response should be valid json. Do not include any headers / trailers like ``` that would prevent
                                the response from being parsed as json.

                                The GitHub handle of the user reading the notifications is `{github_user()}`

                                The notification is for a pull request, this is the raw notification:
                                ```
                                {json.dumps(notification)}
                                ```

                                This is the pull request. Pay special attention to whether the author of the pull
                                request is the current user and tailor your response accordingly:
                                ```
                                {json.dumps(pull_request)}
                                ```

                                And these are the new comments in the pull request:
                                ```
                                {json.dumps(comments)}
                                ```

                                Make a response summarizing what happened in the pull request as part of the new comments in the requested format
                              """
                    )
                ],
            )
        ],
        llm_config,
    )
    parsed_llm_response = parse_llm_response(llm_response)

    return EnhancedNotification(
        id=notification["id"],
        reason=notification["reason"],
        subject=Subject(
            type=SubjectType.PULL_REQUEST,
            title=notification["subject"]["title"],
            url=notification["subject"]["url"],
        ),
        repository=Repository(
            owner=notification["repository"]["owner"]["login"],
            name=notification["repository"]["name"],
        ),
        url=notification["url"],
        updated_at=notification["updated_at"],
        category=category_from_string(parsed_llm_response["category"]),
        priority=priority_from_string(parsed_llm_response["priority"]),
        summary=parsed_llm_response["summary"],
        action_items=parsed_llm_response["action-items"],
        key_points=parsed_llm_response["key-points"],
    )


def process_issue_notification(
    since: Optional[str], before: str, notification: Any
) -> EnhancedNotification:
    issue = get_notification_subject(notification)
    comments = get_notification_subject_comments(since, before, issue)
    llm_response = llm.send(
        [
            llm.Message(
                role=llm.Role.SYSTEM,
                name=None,
                content=[
                    llm.ContentPart_Text(
                        f"""
                                You are used as part of an application to automatically categorize GitHub notifications.
                                Your responses should have the following json structure:

                                ```
                                {model_response_format}
                                ```
                                Your entire response should be valid json. Do not include any headers / trailers like ``` that would prevent
                                the response from being parsed as json.

                                The GitHub handle of the user reading the notifications is `{github_user()}`

                                The notification is for an issue, this is the raw notification:
                                ```
                                {json.dumps(notification)}
                                ```

                                This is the issue. Pay special attention to whether the author of the issue
                                is the current user and tailor your response accordingly:
                                ```
                                {json.dumps(issue)}
                                ```

                                And these are the new comments in this issue:
                                ```
                                {json.dumps(comments)}
                                ```

                                Make a response summarizing what happened in the issue as part of the new comments in the requested format
                              """
                    )
                ],
            )
        ],
        llm_config,
    )
    parsed_llm_response = parse_llm_response(llm_response)

    return EnhancedNotification(
        id=notification["id"],
        reason=notification["reason"],
        subject=Subject(
            type=SubjectType.ISSUE,
            title=notification["subject"]["title"],
            url=notification["subject"]["url"],
        ),
        repository=Repository(
            owner=notification["repository"]["owner"]["login"],
            name=notification["repository"]["name"],
        ),
        url=notification["url"],
        updated_at=notification["updated_at"],
        category=category_from_string(parsed_llm_response["category"]),
        priority=priority_from_string(parsed_llm_response["priority"]),
        summary=parsed_llm_response["summary"],
        action_items=parsed_llm_response["action-items"],
        key_points=parsed_llm_response["key-points"],
    )


def process_check_suite_notification(notification: Any) -> EnhancedNotification:
    return EnhancedNotification(
        id=notification["id"],
        reason=notification["reason"],
        subject=Subject(
            type=SubjectType.CHECK_SUITE,
            title=notification["subject"]["title"],
            url=None,
        ),
        repository=Repository(
            owner=notification["repository"]["owner"]["login"],
            name=notification["repository"]["name"],
        ),
        url=notification["url"],
        updated_at=notification["updated_at"],
        category=Category.CI_CD,
        priority=Priority.NORMAL,
        summary=notification["subject"]["title"],
        action_items=[],
        key_points=[],
    )


def get_notification_subject(notification: Any) -> Any:
    res = request(
        "GET",
        notification["subject"]["url"],
        headers={"authorization": f"Bearer {github_auth_token()}"},
    )
    return res.json()


def get_notification_subject_comments(
    since: Optional[str],
    before: str,
    subject: Any,
) -> list[Any]:
    # comments url might be missing if there are not comments
    if "comments_url" not in subject:
        return []

    # Add support for pagination here if you need to support repos with a lot of comments
    if since is not None:
        comments_url = f"{subject['comments_url']}?since={since}"
    else:
        comments_url = subject["comments_url"]
    res = request(
        "GET", comments_url, headers={"authorization": f"Bearer {github_auth_token()}"}
    )
    comments = res.json()
    return list(filter(lambda c: c["created_at"] <= before, comments))


def relative_github_url(url: str) -> str:
    return url.removeprefix("https://api.github.com")


model_response_format = """
    "category": string, // one of action-needed, fyi, releases, ci-cd
    "priority": string, // one of urgent, high, normal, low
    "summary": string, // summary of the notification and what it is about
    "action-items": array<string>, // list of actions that the recipient of the notification needs to perform
    "key-points": array<string> // most important parts of the notification
"""

model_response_schema = schema.Schema(
    {
        "category": schema.And(
            str, lambda s: s in ["action-needed", "fyi", "releases", "ci-cd"]
        ),
        "priority": schema.And(str, lambda s: s in ["urgent", "high", "normal", "low"]),
        "summary": str,
        "action-items": [str],
        "key-points": [str],
    },
)


def parse_llm_response(response: llm.ChatEvent):
    if not isinstance(response, llm.ChatEvent_Message):
        if isinstance(response, llm.ChatEvent_Error):
            raise ValueError(f"received error from model: {response.value.message}")
        raise ValueError(f"unexpected response from model")

    content = response.value.content[0]

    if not isinstance(content, llm.ContentPart_Text):
        raise ValueError("unexpected content format from model")

    try:
        return model_response_schema.validate(json.loads(content.value))
    except Exception:
        raise ValueError(f"Failed parsing model response: {content.value}")


def category_from_string(value: str) -> Category:
    return {
        "action-needed": Category.ACTION_NEEDED,
        "fyi": Category.FYI,
        "releases": Category.RELEASES,
        "ci-ci": Category.CI_CD,
    }[value]


def priority_from_string(value: str) -> Priority:
    return {
        "urgent": Priority.URGENT,
        "high": Priority.HIGH,
        "normal": Priority.NORMAL,
        "low": Priority.LOW,
    }[value]
