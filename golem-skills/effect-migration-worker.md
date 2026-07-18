# Effect skill migration worker

You are migrating exactly one language-specific Golem coding skill to the Effect SDK.

## Required workflow

1. Read every supplied language variant to understand the behavior the skill must teach.
2. Read the generated Effect application guide and the canary Effect skill.
3. Inspect the pinned `golemcloud/effect-golem` source or documentation for every API you use.
4. Create or repair only the assigned Effect skill.
5. Add explicit `effect:` branches to every language-conditional field in the assigned scenarios.
6. Ensure at least one Effect prompt expects the assigned skill by name.
7. Run only cheap static checks or scenario dry-run validation.

## Constraints

- Preserve behavior, not source-language syntax. Do not mechanically rename TypeScript APIs.
- Use real Effect v4 and `@golemcloud/effect-golem` APIs. Never invent an SDK helper.
- Effect method names and CLI values use TypeScript casing and syntax, but scenario prompts must
  explicitly ask for an Effect-based implementation.
- Extend existing scenario language maps; do not rewrite or alter other languages.
- Do not run the live skill harness. The controller runs it after this Amp thread exits.
- Do not run the bug finder tool.
- Do not edit the migration manifest, controller, common skills, Effect SDK source, or unrelated
  files.
- Do not commit, push, reset, restore, clean, or switch branches.
- If the SDK cannot implement the required behavior, leave the closest useful scoped edits and
  clearly report the missing capability and authoritative evidence.

Finish with a concise summary of the files changed, APIs used, scenario coverage, and any blocker.
