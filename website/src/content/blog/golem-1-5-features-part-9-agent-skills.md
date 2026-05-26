---
title: "Golem 1.5 features — Part 9: Agent skills"
date: "2026-04-18T11:50:00Z"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Engineering Articles"]
slug: "golem-1-5-features-part-9-agent-skills"
originalUrl: "https://blog.vigoo.dev/posts/golem15-part9-skills/"
---

## Introduction

I am publishing brief posts highlighting new features in **Golem 1.5**, releasing late April 2026. This installment assumes reader familiarity with Golem; additional context is available in my [other Golem-related posts](https://blog.vigoo.dev/tags/golem/).

## Skills

Coding agents have become standard development practice. Golem 1.5's templates include `AGENTS.md` and a huge catalog of **agent skills** describing every detail of creating, modifying and testing agents on the Golem platform.

A bootstrap skill explaining the `golem new` command will be available through skills.sh. Language-specific skills then deploy to the application directory based on selection.

### Common and per-language skills

The catalog will feature 10-15 language-independent skills plus approximately 25-30 additional skills for each supported language (TypeScript, Rust, Scala, and MoonBit). Daily CI benchmarks track how well popular coding agents utilize these skills.

### Areas covered

Skills address Golem platform development comprehensively: project creation, dependency management, configuration, agent development, HTTP and MCP exposure, inter-agent communication, databases, external services, AI provider integration, webhooks, advanced features like transactions and snapshotting, and troubleshooting.

Future Golem 1.5+ features will receive immediate agentic development support.

<!-- WebFetch did not return any code blocks for this post; the original article appears to be primarily descriptive prose. Verify against the source if expected to contain skill manifest snippets. -->
