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

//! Checks the vectors of the conversion from 1.6 accounts. No shared code imports accounts: the
//! bridge does, so these tests compare the vectors with hand-written expectations. Every 1.6
//! input record must be in the form of its typed model, which 1.6 and 2.0 share.

use super::account::{Account, AccountId};
use super::account_path::{AccountPath, AccountSegment};
use super::account_usage::{
    AccountUsage, AdminResourceGrant, AdminResourceGrantChangeValue, AdminResourceGrantDimension,
    MonthlyUsageModeTransition, StorageResourceLimitValue,
};
use super::auth::Token;
use super::caller::{AccountAuthentication, AccountPrincipal, Pair};
use super::permission_share::PermissionShare;
use super::plan::Plan;
use super::plan_dimension::{
    DeclaredBounds, DeclaredMax, DimensionBoundsError, DimensionDeclaration,
    InstallationAvailability, PlanDimension, resolve_effective_bounds,
};
use jiff::Timestamp;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use test_r::test;
use uuid::Uuid;

/// The segment of the reserved publisher account `/golem`.
const RESERVED_SEGMENT: &str = "golem";

fn vectors() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test-data/account-conversion/conversion-from-1.6-v1.json"
    )))
    .unwrap()
}

fn text<'a>(value: &'a Value, what: &str) -> &'a str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("{what} is not a string: {value}"))
}

fn list<'a>(value: &'a Value, what: &str) -> &'a Vec<Value> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("{what} is not a list: {value}"))
}

fn uuid(value: &Value) -> Uuid {
    let written = text(value, "a UUID");
    let parsed = Uuid::parse_str(written).unwrap_or_else(|error| panic!("{written}: {error}"));
    assert_eq!(
        parsed.hyphenated().to_string(),
        written,
        "a UUID must be lower case with hyphens"
    );
    parsed
}

/// The numeric GitHub user ID of a login mapping, or `None` when the mapping is missing, is not
/// a GitHub mapping or holds anything but canonical decimal text.
fn github_id(mapping: &Value) -> Option<u64> {
    if mapping["provider"].as_str() != Some("github") {
        return None;
    }
    let written = mapping["externalId"].as_str()?;
    let parsed: u64 = written.parse().ok()?;
    (parsed.to_string() == written).then_some(parsed)
}

/// Reads `value` as `T` and checks that it is exactly the JSON form of `T`: no unknown,
/// misspelled or missing field and no unknown enum value.
fn typed<T: Serialize + DeserializeOwned>(value: &Value, what: &str) -> T {
    let parsed: T = serde_json::from_value(value.clone())
        .unwrap_or_else(|error| panic!("{what} is not in its 1.6 form: {error}: {value}"));
    assert_eq!(
        &serde_json::to_value(&parsed).unwrap(),
        value,
        "{what} is not exactly in its 1.6 form"
    );
    parsed
}

fn typed_list<T: Serialize + DeserializeOwned>(value: &Value, what: &str) -> Vec<T> {
    list(value, what)
        .iter()
        .map(|item| typed(item, what))
        .collect()
}

/// The 2.0 dimension of a 1.6 override.
fn grant_dimension(dimension: AdminResourceGrantDimension) -> PlanDimension {
    match dimension {
        AdminResourceGrantDimension::MonthlyComputeGcu => PlanDimension::MonthlyComputeGcu,
        AdminResourceGrantDimension::MonthlyMemoryGbSeconds => {
            PlanDimension::MonthlyMemoryGbSeconds
        }
        AdminResourceGrantDimension::MonthlyDurableStorageGbMonth => {
            PlanDimension::MonthlyDurableStorageGbMonth
        }
        AdminResourceGrantDimension::MonthlyEphemeralStorageGbMonth => {
            PlanDimension::MonthlyEphemeralStorageGbMonth
        }
        AdminResourceGrantDimension::MaxMemoryPerAgent => PlanDimension::MaxMemoryPerAgent,
        AdminResourceGrantDimension::MaxStoragePerAgent => PlanDimension::MaxStoragePerAgent,
    }
}

/// The GitHub user ID of an account's person, `None` when the export proves that the account
/// has no external login, or the refusal of its login evidence. The evidence is the account's
/// exported 1.6 `oauth2_tokens` rows: a missing or null list proves nothing, and more than one
/// mapping or a mapping that is not a canonical GitHub mapping is inconsistent.
fn person_of(entry: &Value) -> Result<Option<u64>, &'static str> {
    let Some(mappings) = entry.get("loginMappings").and_then(Value::as_array) else {
        return Err("missingLoginEvidence");
    };
    match mappings.as_slice() {
        [] => Ok(None),
        [mapping] => github_id(mapping).map(Some).ok_or("invalidMapping"),
        _ => Err("invalidMapping"),
    }
}

fn account_pair(golem_issuer: &str, account_id: Uuid) -> String {
    AccountPrincipal {
        issuer: golem_issuer.to_string(),
        account_id: AccountId(account_id),
        authentication: AccountAuthentication::SignIn,
    }
    .pair()
    .to_string()
}

fn plans(vectors: &Value, extra: &[Value]) -> BTreeMap<Uuid, Plan> {
    list(&vectors["plans"], "plans")
        .iter()
        .map(|case| &case["plan"])
        .chain(extra)
        .map(|plan| {
            let plan: Plan = typed(plan, "a plan");
            (plan.plan_id.0, plan)
        })
        .collect()
}

fn declared_max(value: DeclaredMax) -> Value {
    match value {
        DeclaredMax::Finite(value) => json!({ "finite": value }),
        DeclaredMax::Unlimited => json!("unlimited"),
    }
}

fn declaration(declaration: DimensionDeclaration) -> Value {
    json!({
        "min": declaration.bounds.min,
        "max": declaration.bounds.max.map(declared_max),
        "operating": declaration.operating.map(|operating| json!({
            "default": declared_max(operating.default),
            "userConfigurable": operating.user_configurable,
        })),
    })
}

/// Whether a declaration is valid on its own: its bounds lie in the dimension's domain and
/// its operating default lies within them.
fn valid_declaration(dimension: PlanDimension, declaration: DimensionDeclaration) -> bool {
    let available = InstallationAvailability::Available;
    if let Err(error) = resolve_effective_bounds(dimension, available, &[Some(declaration.bounds)])
        && error != DimensionBoundsError::MissingMaximum
    {
        return false;
    }
    let Some(operating) = declaration.operating else {
        return true;
    };
    // An unlimited default never tightens a finite ceiling, so the resolution accepts it.
    if operating.default == DeclaredMax::Unlimited
        && matches!(declaration.bounds.max, Some(DeclaredMax::Finite(_)))
    {
        return false;
    }
    let default_level = DeclaredBounds {
        min: match operating.default {
            DeclaredMax::Finite(value) => Some(value),
            DeclaredMax::Unlimited => None,
        },
        max: Some(operating.default),
    };
    resolve_effective_bounds(
        dimension,
        available,
        &[Some(declaration.bounds), Some(default_level)],
    )
    .is_ok()
}

/// The refusals that an import of `accounts` must report, read from its inputs.
fn refusals(
    vectors: &Value,
    accounts: &[Value],
    extra_plans: &[Value],
    extra_actors: &Value,
) -> BTreeSet<String> {
    let plans = plans(vectors, extra_plans);
    let imported: BTreeSet<Uuid> = accounts
        .iter()
        .map(|entry| uuid(&entry["account"]["id"]))
        .collect();
    let mut mapped_actors = vectors["actors"].as_object().unwrap().clone();
    mapped_actors.extend(extra_actors.as_object().unwrap().clone());
    let mut found = BTreeSet::new();
    let mut refuse = |refusal: &str| {
        found.insert(refusal.to_string());
    };

    let mut ids = BTreeSet::new();
    let mut segments = BTreeMap::new();
    for entry in accounts {
        let account: Account = typed(&entry["account"], "an account");
        typed::<AccountUsage>(&entry["usage"], "usage");
        typed_list::<Token>(&entry["tokens"], "a token");
        let id = account.id.0;
        if !ids.insert(id) {
            refuse("duplicateMapping");
            continue;
        }
        let segment = text(&entry["segment"], "segment");
        match AccountSegment::from_str(segment) {
            Err(_) => refuse("invalidSegment"),
            Ok(_) if segment == RESERVED_SEGMENT => refuse("reservedPath"),
            Ok(_) => {
                if segments.insert(segment.to_string(), id).is_some() {
                    refuse("duplicateSibling");
                }
            }
        }
        if let Err(refusal) = person_of(entry) {
            refuse(refusal);
        }
        for share in typed_list::<PermissionShare>(&entry["shares"], "a share") {
            assert_eq!(
                share.owner_account_id.0, id,
                "a share is listed under its owner"
            );
            if share.current_card_id.is_none() {
                refuse("parentlessShare");
            }
            if !imported.contains(&share.target_account_id.0) {
                refuse("shareOutsideImport");
            }
        }
        let grants = typed_list::<AdminResourceGrant>(&entry["adminGrants"], "a grant");
        let transitions =
            typed_list::<MonthlyUsageModeTransition>(&entry["modeHistory"], "a transition");
        let actors = grants
            .iter()
            .map(|grant| grant.actor_account_id.0)
            .chain(transitions.iter().map(|change| change.actor_account_id.0));
        for actor in actors {
            let mapped = mapped_actors
                .get(&actor.to_string())
                .and_then(github_id)
                .is_some();
            if !imported.contains(&actor) && !mapped {
                refuse("unmappedActor");
            }
        }
        // 1.6 stores a disabled storage grant as a sentinel; 2.0 has no disabled override.
        if grants.iter().any(|grant| {
            matches!(
                AdminResourceGrantChangeValue::from_raw(grant.dimension, grant.value),
                AdminResourceGrantChangeValue::Disabled(_)
            )
        }) {
            refuse("invalidBounds");
        }
        let plan = &plans[&account.plan_id.0];
        // Storage is either disabled in both values or in neither.
        let disabled = |value: StorageResourceLimitValue| {
            matches!(value, StorageResourceLimitValue::Disabled(_))
        };
        if disabled(plan.max_storage_per_agent) != disabled(plan.max_storage_per_agent_ceiling)
            || PlanDimension::ALL.into_iter().any(|dimension| {
                plan.dimension_declaration(dimension)
                    .is_some_and(|declared| !valid_declaration(dimension, declared))
            })
        {
            refuse("invalidBounds");
        }
    }
    found
}

#[test]
fn global_plans_keep_their_values_under_the_root() {
    let vectors = vectors();
    for case in list(&vectors["plans"], "plans") {
        let name = text(&case["name"], "name");
        let plan: Plan = typed(&case["plan"], "a plan");
        let expected = &case["expected"];
        assert_eq!(expected["owner"], "/", "{name}");
        assert_eq!(expected["name"], plan.name.0, "{name}");
        assert_eq!(
            expected["overageAllowedByPlan"], plan.overage_allowed_by_plan,
            "{name}"
        );
        let availability = match plan.storage_availability() {
            InstallationAvailability::Available => json!("available"),
            InstallationAvailability::Unavailable(reason) => serde_json::to_value(reason).unwrap(),
        };
        assert_eq!(expected["storageAvailability"], availability, "{name}");

        let declared: BTreeMap<String, Value> = PlanDimension::ALL
            .into_iter()
            .filter_map(|dimension| {
                plan.dimension_declaration(dimension)
                    .map(|declared| (format!("{dimension:?}"), declaration(declared)))
            })
            .collect();
        let expected: BTreeMap<String, Value> = expected["declarations"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(dimension, value)| (dimension.clone(), value.clone()))
            .collect();
        assert_eq!(declared, expected, "{name}");
        // A 1.6 plan never declares the total agent count.
        assert!(!declared.contains_key("TotalAgentCount"), "{name}");
    }
}

#[test]
fn the_plan_vectors_cover_the_value_traps() {
    let vectors = vectors();
    let plans: Vec<Plan> = list(&vectors["plans"], "plans")
        .iter()
        .map(|case| typed(&case["plan"], "a plan"))
        .collect();
    // A conversion that clips the largest blob limit or reads it as unlimited.
    assert!(plans.iter().any(|plan| plan.blob_storage_limit == u64::MAX));
    // A conversion that keeps the runtime sentinel as a finite concurrency bound.
    assert!(
        plans
            .iter()
            .any(|plan| plan.max_concurrent_agents_per_executor >= 1_000_000_000_000_000_000)
    );
    // The same for the oplog write rate.
    assert!(
        plans
            .iter()
            .any(|plan| plan.oplog_writes_per_second >= 1_000_000_000_000_000_000)
    );
    // One plan has managed storage, the other has none.
    let availability: BTreeSet<bool> = plans
        .iter()
        .map(|plan| plan.storage_availability() == InstallationAvailability::Available)
        .collect();
    assert_eq!(availability, BTreeSet::from([false, true]));
}

#[test]
fn imports_map_accounts_people_and_evidence() {
    let vectors = vectors();
    let golem_issuer = text(&vectors["golemIssuer"], "golemIssuer");
    let actors = vectors["actors"].as_object().unwrap();
    for case in list(&vectors["imports"], "imports") {
        let name = text(&case["name"], "name");
        let accounts = list(&case["accounts"], "accounts");
        let expected = list(&case["expected"], "expected");
        assert!(
            refusals(&vectors, accounts, &[], &json!({})).is_empty(),
            "{name}"
        );
        assert_eq!(accounts.len(), expected.len(), "{name}");
        let plans = plans(&vectors, &[]);
        let path_of: BTreeMap<Uuid, String> = accounts
            .iter()
            .map(|entry| {
                let segment = AccountSegment::from_str(text(&entry["segment"], "segment")).unwrap();
                let path = AccountPath::root().child(segment).unwrap().to_string();
                (uuid(&entry["account"]["id"]), path)
            })
            .collect();
        let actor_pair = |actor: Uuid| -> String {
            if path_of.contains_key(&actor) {
                account_pair(golem_issuer, actor)
            } else {
                Pair::github_user(github_id(&actors[&actor.to_string()]).unwrap()).to_string()
            }
        };

        for (entry, expected) in accounts.iter().zip(expected) {
            let account = &entry["account"];
            let id = uuid(&account["id"]);
            let typed_account: Account = typed(account, "an account");
            assert_eq!(typed_account.id.0, id, "{name}");
            let path = &path_of[&id];
            let person = person_of(entry)
                .unwrap()
                .map(|id| Pair::github_user(id).to_string());
            assert_eq!(uuid(&expected["accountId"]), id, "{name}");
            assert_eq!(expected["path"], path.as_str(), "{name}");
            let depth = AccountPath::from_str(path).unwrap().depth();
            assert_eq!(expected["depth"], depth, "{name}");
            assert_eq!(expected["displayName"], account["name"], "{name}");
            assert_eq!(
                expected["accountPair"],
                account_pair(golem_issuer, id),
                "{name}"
            );
            assert_eq!(expected["personPair"], json!(person), "{name}");
            // Person effects apply only where the source has a valid login mapping.
            let effects = if person.is_some() { "mapped" } else { "none" };
            assert_eq!(expected["personEffects"], effects, "{name}");
            // A stored `golem-user` principal of the account, as Identity D6 converts it.
            let reference = person
                .clone()
                .unwrap_or_else(|| account_pair(golem_issuer, id));
            assert_eq!(expected["principalReference"], reference, "{name}");
            let plan = &plans[&uuid(&account["planId"])];
            assert_eq!(
                expected["plan"],
                json!({ "name": plan.name.0, "owner": "/" }),
                "{name}"
            );
            let card = person
                .as_ref()
                .map(|person| json!({ "wallet": person, "account": path, "derivedFrom": "/" }));
            assert_eq!(expected["rootManagementCard"], json!(card), "{name}");
            assert_eq!(expected["removedRoles"], account["roles"], "{name}");
            assert_eq!(
                expected["monthlyUsageMode"], entry["monthlyUsageMode"],
                "{name}"
            );

            let history = list(&entry["modeHistory"], "modeHistory");
            let consent = if history.is_empty() {
                "none"
            } else {
                "history"
            };
            assert_eq!(expected["consentEvidence"], consent, "{name}");
            let history: Vec<Value> = history
                .iter()
                .map(|transition| {
                    let mut converted = transition.clone();
                    let actor = uuid(&transition["actorAccountId"]);
                    let object = converted.as_object_mut().unwrap();
                    object.remove("actorAccountId");
                    object.insert("actorPair".to_string(), json!(actor_pair(actor)));
                    converted
                })
                .collect();
            assert_eq!(expected["modeHistory"], json!(history), "{name}");

            let typed_usage: AccountUsage = typed(&entry["usage"], "usage");
            assert_eq!(typed_usage.account_id.0, id, "{name}");
            let mut usage = entry["usage"]["usage"].clone();
            usage.as_object_mut().unwrap().remove("metering");
            assert_eq!(expected["usage"], usage, "{name}");

            let overrides: Vec<Value> =
                typed_list::<AdminResourceGrant>(&entry["adminGrants"], "a grant")
                    .iter()
                    .zip(list(&entry["adminGrants"], "adminGrants"))
                    .map(|(grant, written)| {
                        let value = match AdminResourceGrantChangeValue::from_raw(
                            grant.dimension,
                            grant.value,
                        ) {
                            AdminResourceGrantChangeValue::Finite(value) => {
                                json!({ "finite": value.value })
                            }
                            AdminResourceGrantChangeValue::Unlimited(_) => json!("unlimited"),
                            AdminResourceGrantChangeValue::Disabled(_) => {
                                panic!("a disabled grant is refused")
                            }
                        };
                        json!({
                            "dimension": format!("{:?}", grant_dimension(grant.dimension)),
                            "value": value,
                            "reason": written["reason"],
                            "actorPair": actor_pair(grant.actor_account_id.0),
                            "grantedAt": written["grantedAt"],
                            "expiresAt": written["expiresAt"],
                        })
                    })
                    .collect();
            assert_eq!(expected["overrides"], json!(overrides), "{name}");

            let tokens: Vec<Value> = typed_list::<Token>(&entry["tokens"], "a token")
                .iter()
                .zip(list(&entry["tokens"], "tokens"))
                .map(|(token, written)| {
                    json!({
                        "id": written["id"],
                        "authenticates": account_pair(golem_issuer, token.account_id.0),
                        "expiresAt": written["expiresAt"],
                    })
                })
                .collect();
            assert_eq!(expected["tokens"], json!(tokens), "{name}");

            let shares: Vec<Value> = typed_list::<PermissionShare>(&entry["shares"], "a share")
                .iter()
                .zip(list(&entry["shares"], "shares"))
                .map(|(share, written)| {
                    json!({
                        "id": written["id"],
                        "owner": path_of[&share.owner_account_id.0],
                        "target": path_of[&share.target_account_id.0],
                        "name": written["name"],
                        "data": written["data"],
                        "conversion": "cards",
                    })
                })
                .collect();
            assert_eq!(expected["shares"], json!(shares), "{name}");
        }
    }
}

/// One person with two accounts keeps two account authorities and two root cards; the
/// accounts differ in plan, mode and usage.
#[test]
fn the_import_vectors_cover_one_person_with_two_accounts() {
    let vectors = vectors();
    let import = &list(&vectors["imports"], "imports")[0];
    let expected = list(&import["expected"], "expected");
    let field = |name: &str| -> BTreeSet<String> {
        expected
            .iter()
            .map(|account| account[name].to_string())
            .collect()
    };
    assert_eq!(field("personPair").len(), 1);
    assert_eq!(field("accountPair").len(), 2);
    assert_eq!(field("rootManagementCard").len(), 2);
    assert_eq!(field("plan").len(), 2);
    assert_eq!(field("monthlyUsageMode").len(), 2);
    assert_eq!(field("depth"), BTreeSet::from(["1".to_string()]));
    // Fractional usage read strictly inside its UTC month, before the month ends.
    assert!(expected.iter().any(|account| {
        let usage = &account["usage"];
        let compute = usage["computeGcu"].as_f64().unwrap();
        let year = usage["period"]["year"].as_i64().unwrap();
        let month = usage["period"]["month"].as_i64().unwrap();
        let (next_year, next_month) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
        let start: Timestamp = format!("{year:04}-{month:02}-01T00:00:00Z")
            .parse()
            .unwrap();
        let end: Timestamp = format!("{next_year:04}-{next_month:02}-01T00:00:00Z")
            .parse()
            .unwrap();
        let as_of: Timestamp = text(&usage["asOf"], "asOf").parse().unwrap();
        compute.fract() != 0.0 && start < as_of && as_of < end
    }));
    // A display name that is not a valid segment, so a conversion must not reuse it as one.
    assert!(expected.iter().all(|account| {
        AccountSegment::from_str(account["displayName"].as_str().unwrap()).is_err()
    }));
}

/// The vectors hold both outcomes: an account with a valid GitHub mapping, whose person gets
/// the root-management card, and an account whose export proves it has no login, which gets no
/// person, no person-held card and no grant beyond its own.
#[test]
fn the_import_vectors_cover_both_person_outcomes() {
    let vectors = vectors();
    let accounts: Vec<&Value> = list(&vectors["imports"], "imports")
        .iter()
        .flat_map(|import| list(&import["expected"], "expected"))
        .collect();
    let effects: BTreeSet<&str> = accounts
        .iter()
        .map(|account| text(&account["personEffects"], "personEffects"))
        .collect();
    assert_eq!(effects, BTreeSet::from(["mapped", "none"]));
    for account in accounts {
        if account["personEffects"] == "none" {
            assert!(account["personPair"].is_null());
            assert!(account["rootManagementCard"].is_null());
            assert_eq!(account["principalReference"], account["accountPair"]);
        }
    }
}

#[test]
fn refusals_match_their_inputs() {
    let vectors = vectors();
    let mut covered = BTreeSet::new();
    for case in list(&vectors["refusals"], "refusals") {
        let name = text(&case["name"], "name");
        let refusal = text(&case["refusal"], "refusal");
        let found = refusals(
            &vectors,
            list(&case["accounts"], "accounts"),
            list(&case["extraPlans"], "extraPlans"),
            &case["extraActors"],
        );
        assert_eq!(found, BTreeSet::from([refusal.to_string()]), "{name}");
        covered.insert(refusal);
    }
    for required in [
        "invalidSegment",
        "reservedPath",
        "duplicateSibling",
        "duplicateMapping",
        "invalidMapping",
        "missingLoginEvidence",
        "parentlessShare",
        "shareOutsideImport",
        "unmappedActor",
        "invalidBounds",
    ] {
        assert!(
            covered.contains(required),
            "no refusal vector for {required}"
        );
    }
}
