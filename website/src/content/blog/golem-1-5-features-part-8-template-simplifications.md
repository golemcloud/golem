---
title: "Golem 1.5 features — Part 8: Template simplifications and automatic updates"
date: "2026-04-18T11:45:00Z"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Engineering Articles"]
slug: "golem-1-5-features-part-8-template-simplifications"
originalUrl: "https://blog.vigoo.dev/posts/golem15-part8-template-simplifications/"
---

## Introduction

I am writing a series of _short_ posts showcasing the new features of **Golem 1.5**, to be released at the end of April, 2026. The episodes of this series will be short and assume the reader knows what Golem is. Check my [other Golem-related posts](https://blog.vigoo.dev/tags/golem/) for more information!

Parts released so far:

- [Part 1: Code-first routes](https://blog.vigoo.dev/posts/golem15-part1-code-first-routes)
- [Part 2: Webhooks](https://blog.vigoo.dev/posts/golem15-part2-webhooks)
- [Part 3: MCP](https://blog.vigoo.dev/posts/golem15-part3-mcp)
- [Part 4: Node.js compatibility](https://blog.vigoo.dev/posts/golem15-part4-nodejs)
- [Part 5: Scala support](https://blog.vigoo.dev/posts/golem15-part5-scala)
- [Part 6: User-defined snapshotting](https://blog.vigoo.dev/posts/golem15-part6-user-defined-snapshotting)
- [Part 7: Configuration and Secrets](https://blog.vigoo.dev/posts/golem15-part7-config-and-secrets)
- [Part 8: Template simplifications and automatic updates](https://blog.vigoo.dev/posts/golem15-part8-template-simplifications)
- [Part 9: Agent skills](https://blog.vigoo.dev/posts/golem15-part9-skills)
- [Part 11: Bridge libraries](https://blog.vigoo.dev/posts/golem15-part11-bridges)
- [Part 12: REPL](https://blog.vigoo.dev/posts/golem15-part12-repl)
- [Part 13: Per-agent configuration](https://blog.vigoo.dev/posts/golem15-part13-per-agent-config)
- [Part 14: OpenTelemetry](https://blog.vigoo.dev/posts/golem15-part14-otlp)
- [Part 15: MoonBit](https://blog.vigoo.dev/posts/golem15-part15-moonbit)
- [Part 16: Quotas](https://blog.vigoo.dev/posts/golem15-part16-quotas)
- [Part 17: Semantic retry policies](https://blog.vigoo.dev/posts/golem15-part17-semantic-retry-policies)

## Golem templates

Golem comes with **built-in templates** for the supported languages since 1.0. These templates not only contained simple examples of defining an agent running on Golem, but a large amount of build infrastructure as well, and a structure we created to make it easy for a Golem application to grow to be a big multi-component, multi-language project with additional user defined shared libraries and so on.

### Previous versions

Although these old templates were quite powerful, they generated a lot of "noise" for reasons that many Golem applications never required. It was hard to understand what all these files are, where we should add our own code, what to touch and what to not touch.

In addition to that, creating a project with `golem new` applied that particular golem version's understanding of how a Golem application has to be built. If later the user updated golem itself, the process of updating the build definitions to match the new version was extremely hard.

### The new templates

We understood these problems and in **Golem 1.5** we have a fully revised way of how these templates work!

For example, creating a new **TypeScript** project gives us the following:

```bash
$ golem new --template ts/default ts-example
$ ls ts-example
╭───┬──────────────────────────┬──────┬─────────┬─────────────────────╮
│ # │           name           │ type │  size   │      modified       │
├───┼──────────────────────────┼──────┼─────────┼─────────────────────┤
│ 0 │ ts-example/AGENTS.md     │ file │ 15.4 kB │ 2026-04-18 11:19:10 │
│ 1 │ ts-example/golem.yaml    │ file │   997 B │ 2026-04-18 11:19:10 │
│ 2 │ ts-example/package.json  │ file │   482 B │ 2026-04-18 11:19:10 │
│ 3 │ ts-example/src           │ dir  │   128 B │ 2026-04-18 11:19:10 │
│ 4 │ ts-example/tsconfig.json │ file │   578 B │ 2026-04-18 11:19:10 │
╰───┴──────────────────────────┴──────┴─────────┴─────────────────────╯
$ ls ts-example/src
╭───┬─────────────────────────────────┬──────┬───────┬─────────────────────╮
│ # │              name               │ type │ size  │      modified       │
├───┼─────────────────────────────────┼──────┼───────┼─────────────────────┤
│ 0 │ ts-example/src/counter-agent.ts │ file │ 593 B │ 2026-04-18 11:19:10 │
│ 1 │ ts-example/src/main.ts          │ file │  26 B │ 2026-04-18 11:19:10 │
╰───┴─────────────────────────────────┴──────┴───────┴─────────────────────╯
```

The root of our application contains our application's `golem.yaml` — this is where we can configure things like HTTP deployments, configuration, etc. Other than that we just see a simple `package.json` and `tsconfig.json` and the two TypeScript source files of the _default_ example.

### Mixing multiple templates

In the new template system we can also mix-in multiple templates into a single application, either during `golem new` or later. These additional templates just bring in more example agents — for example we could have the default CounterAgent and next to it a "human in the loop" example. Our new templates are composable.

### Automatic updates

Another big problem with the old template system was that we could not easily update Golem applications to a new Golem version. New versions usually require updating the language-specific Golem SDKs, and sometimes also require changes to the build steps that `golem build` performs under the hood. With the new templates, as seen above, these build steps are no longer dumped to static files generated by `golem new`. They get dynamically extracted by the CLI tool to temporary directories, which means they can still be observed for troubleshooting purposes but they don't get checked into the application's repository, and a new golem version can just use an updated set of these build specific files if needed.

Golem-specific dependencies in the project's `package.json`/`Cargo.toml`/etc also get verified by the `golem build` command, and if anything needs to be updated, it is going to be reported with a helpful message.

The automatic update also applies to [our agent skills](https://blog.vigoo.dev/posts/golem15-part9-skills). Simply updating the CLI will guarantee the project always has the latest version of the golem-specific agent skills.
