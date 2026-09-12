// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

/// SQL identifiers and record projection for a release repository.
///
/// Implementations are private, compile-time descriptions. Consequently the identifiers used by
/// the query builders below can never originate in a request.
pub(crate) trait ReleaseKind {
    const RELEASE_TABLE: &'static str;
    const RELEASE_ID: &'static str;
    const NAME: &'static str;
    const VERSION: &'static str;
    const GRANT_TABLE: &'static str;
    const SELECT: &'static str;
}

pub(crate) trait GrantKind: ReleaseKind {
    const GRANT_ID: &'static str;
}

pub(crate) fn release_by_id<K: ReleaseKind>() -> String {
    format!("{} WHERE tr.{} = $1", K::SELECT, K::RELEASE_ID)
}

pub(crate) fn release_by_coordinates<K: ReleaseKind>(superseded: i16) -> String {
    format!(
        "{} WHERE tr.owner_account_id = $1 AND tr.{} = $2 AND tr.{} = $3 ORDER BY tr.lifecycle = {}, tr.created_at DESC LIMIT 1",
        K::SELECT,
        K::NAME,
        K::VERSION,
        superseded
    )
}

pub(crate) fn releases_by_owner<K: ReleaseKind>() -> String {
    format!(
        "{} WHERE tr.owner_account_id = $1 ORDER BY tr.{}, tr.{}",
        K::SELECT,
        K::NAME,
        K::VERSION
    )
}

pub(crate) fn strict_following_grant_exists<K: ReleaseKind>() -> String {
    format!(
        r#"SELECT 1
           FROM {} etg
           JOIN environments e ON e.environment_id = etg.environment_id
           JOIN environment_revisions er
             ON er.environment_id = e.environment_id
            AND er.revision_id = e.current_revision_id
          WHERE etg.{} = $1
            AND etg.follow_coordinates
            AND etg.deleted_at IS NULL
            AND e.deleted_at IS NULL
            AND er.version_check
          LIMIT 1"#,
        K::GRANT_TABLE,
        K::RELEASE_ID
    )
}

pub(crate) fn lock_published_release<K: ReleaseKind>() -> String {
    format!(
        "UPDATE {} SET lifecycle = lifecycle WHERE {} = $1 AND lifecycle = $2",
        K::RELEASE_TABLE,
        K::RELEASE_ID
    )
}

pub(crate) fn release_lifecycle<K: ReleaseKind>() -> String {
    format!(
        "SELECT lifecycle FROM {} WHERE {} = $1",
        K::RELEASE_TABLE,
        K::RELEASE_ID
    )
}

pub(crate) fn supersede_release<K: ReleaseKind>() -> String {
    format!(
        "UPDATE {} SET lifecycle = $2, state_changed_at = $3, state_changed_by = $4 WHERE {} = $1 AND lifecycle != $2 AND origin != $5",
        K::RELEASE_TABLE,
        K::RELEASE_ID
    )
}

pub(crate) fn move_following_grants<K: ReleaseKind>() -> String {
    format!(
        "UPDATE {} SET {} = $2, state_changed_at = $3, state_changed_by = $4 WHERE {} = $1 AND follow_coordinates AND deleted_at IS NULL",
        K::GRANT_TABLE,
        K::RELEASE_ID,
        K::RELEASE_ID
    )
}

pub(crate) fn change_release_lifecycle<K: ReleaseKind>(returning: bool) -> String {
    format!(
        "UPDATE {} SET lifecycle = $2, state_changed_at = $3, state_changed_by = $4 WHERE {} = $1 AND lifecycle = $5 AND origin != $6{}",
        K::RELEASE_TABLE,
        K::RELEASE_ID,
        if returning {
            format!(" RETURNING {}", K::RELEASE_ID)
        } else {
            String::new()
        }
    )
}

pub(crate) fn delete_grant<K: GrantKind>() -> String {
    format!(
        "UPDATE {} SET state_changed_at = $2, state_changed_by = $3, deleted_at = $2, deleted_by = $3 WHERE {} = $1 AND deleted_at IS NULL AND NOT protected AND (NOT $4 OR automatic) RETURNING {}",
        K::GRANT_TABLE,
        K::GRANT_ID,
        K::GRANT_ID
    )
}

pub(crate) fn set_grant_management<K: GrantKind>() -> String {
    format!(
        "UPDATE {} SET automatic = $4, follow_coordinates = $5, state_changed_at = $6, state_changed_by = $7 WHERE {} = $1 AND environment_id = $2 AND {} = $3 AND deleted_at IS NULL AND NOT protected AND (NOT $4 OR automatic)",
        K::GRANT_TABLE,
        K::GRANT_ID,
        K::RELEASE_ID
    )
}

pub(crate) fn restore_grant<K: GrantKind>(protected: bool) -> String {
    let management = if protected {
        String::new()
    } else {
        ", automatic = $6, follow_coordinates = COALESCE($7, follow_coordinates)".to_string()
    };
    format!(
        "UPDATE {} SET state_changed_at = $4, state_changed_by = $5{} , deleted_at = NULL, deleted_by = NULL WHERE {} = $1 AND environment_id = $2 AND {} = $3 AND deleted_at IS NOT NULL AND {}protected",
        K::GRANT_TABLE,
        management,
        K::GRANT_ID,
        K::RELEASE_ID,
        if protected { "" } else { "NOT " }
    )
}
