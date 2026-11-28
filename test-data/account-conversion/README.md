# Account conversion vectors

Seeded vectors for the conversion of 1.6 accounts, people, plans, usage, consent, overrides,
tokens and shares to the 2.0 account tree. The bridge import and its fixtures (B10, B11) check
themselves against these files. No shared code implements the import: the bridge does, so the
Rust tests compare the vectors with hand-written expectations.

Every expected value is written by hand from the map below, not computed from the inputs.

## `conversion-from-1.6-v1.json`

- `golemIssuer`: the installation's Golem issuer.
- `actors`: the GitHub login mapping of each 1.6 actor account that is not imported itself,
  such as an administrator who granted an override. Its person pair follows the login mapping
  row below.
- `plans`: each global 1.6 plan in its JSON form, and the 2.0 definition it becomes: its owner,
  its name, its storage availability and one typed declaration per dimension.
- `imports`: sets of 1.6 accounts, each with its approved `segment`, its
  `loginMappings` (the account's exported 1.6 `oauth2_tokens` rows), its monthly usage mode and mode history, its current-period usage, its
  overrides (`adminGrants`), its tokens without secrets and its shares, and the 2.0 result for
  each account.
Every 1.6 record (account, plan, usage, mode transition, override, token and share) is exactly
in the JSON form of its typed model in `golem-common`, which 1.6 and 2.0 share; the tests read
each one as that type and write it back.

- `refusals`: inputs that the import must refuse without repair, and the one refusal each one
  gives. `extraPlans` and `extraActors` add plans and actors that only that case uses.

Dimension names in the output are the names of the 2.0 plan dimensions, such as
`MonthlyComputeGcu`. A share is listed under its owner account.

## The map

| 1.6 input | 2.0 result |
|---|---|
| Account | A direct child of `/` at `/<segment>`, depth 1, with the same account ID. The segment comes from the approved list, never from the old name. |
| Account name | The display name, unchanged. |
| Email | Nothing. An email is not identity. |
| One login mapping | The person pair `("https://github.com","<numeric GitHub user ID>")`, never the login name. Only GitHub mappings are imported. `personEffects` is `mapped`. |
| No login mapping (`loginMappings: []`) | The export proves the account has no external login. The account imports with its identity, tokens and provable authority, and creates no person, login mapping, person-held card or grant. `personPair` and `rootManagementCard` are `null`, and `personEffects` is `none`. |
| Stored `golem-user` principal | `principalReference`: the person pair with one login mapping, the account pair without one (Identity D6). |
| Account authority | The account pair `("<Golem issuer>","account:<account-id>")`. One person with two accounts keeps two separate account authorities. |
| Account root card | With one login mapping, a root-management card in the person's wallet for the account's path, derived from `/`. Without one, none. |
| Roles | Removed and listed in `removedRoles`; a role grants nothing in 2.0. |
| Global plan | A definition with the same name owned by `/`. Each value keeps its unit and amount, without clipping: `u64::MAX` stays finite. A per-executor concurrency or an oplog write rate at or above `10^18` is unlimited. Disabled storage is installation availability, not a bound. No 1.6 plan declares the total agent count. |
| Monthly usage mode | Unchanged. `hardLimit` is the import's value only when the account had it. |
| Mode history | Unchanged, with each actor as a pair. An account without history has `consentEvidence: none`; no consent is invented. |
| Usage | The values of the source UTC period, fractions included, at the source `asOf`. Metering status is not imported. |
| Override | The same dimension, amount, reason, times and expiry. A memory grant at `u64::MAX` or `10^18` is unlimited, as in 1.6. Every other amount is finite. The actor becomes a pair: the account pair of an imported actor, the person pair of an actor in `actors`. |
| Token | The same ID and expiry. It authenticates the account pair, not the person. |
| Share with a parent card | Converted to cards from the owner's path to the target's path, with the same name and exactly the same permission data. The 1.6 share revision is not kept. |

## Refusals

| Refusal | Input |
|---|---|
| `invalidSegment` | A segment that is not a valid account path segment: upper case, `--`, a hyphen at an edge, an encoded separator. |
| `reservedPath` | The segment `golem`, the reserved publisher account. |
| `duplicateSibling` | Two accounts with the same segment. |
| `duplicateMapping` | One 1.6 account listed twice. |
| `invalidMapping` | More than one login mapping, a mapping of another provider, or a GitHub ID that is not canonical decimal text, such as a login name or a leading zero. |
| `missingLoginEvidence` | No `loginMappings` in the export, or `null`: absence is not proven, so the account is not imported as account-only. |
| `parentlessShare` | A share without a current card. |
| `shareOutsideImport` | A share whose target is not in the same import. |
| `unmappedActor` | An override or a transition by an actor that is neither imported nor in `actors` with a valid GitHub mapping. |
| `invalidBounds` | A plan declaration outside its own bounds, such as a memory default above its ceiling or an unlimited default under a finite ceiling, or storage that is disabled in only one of its default and ceiling, or a storage grant at or above `10^16`, which 1.6 reads as disabled and 2.0 cannot express as an override. |

## Open

Q10, the first external start and the reconciliation of already-paid orders, is not decided.
These vectors keep the source period and usage and state no allowance for the next period.

## Consumers

- Rust: `golem-common/src/base_model/account_conversion_tests.rs`.

When the map changes, change these files in the same commit; the Rust tests fail until both
agree.
