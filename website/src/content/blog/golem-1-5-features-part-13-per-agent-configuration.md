---
title: "Golem 1.5 features — Part 13: Per-agent configuration"
date: "2026-04-21T00:00:00Z"
author: "Daniel Vigovszky"
tags: ["Product Updates", "Engineering Articles"]
slug: "golem-1-5-features-part-13-per-agent-configuration"
originalUrl: "https://blog.vigoo.dev/posts/golem15-part13-per-agent-config/"
---

## Introduction

I am writing a series of posts about Golem 1.5's new features, releasing at the end of April 2026. This installment assumes reader familiarity with Golem. Check the [other Golem-related posts](https://blog.vigoo.dev/tags/golem/) for more information.

## Components vs agents

Golem 1.5 shifts focus from components to agents as the primary user-defined entities. While components remain the compilation and deployment unit, each can now contain multiple agents. The update moves customization settings — environment variables, file systems, configuration, and bridge generation — to the agent level rather than the component level.

This change aligns the manifest structure with how we can configure some other aspects (such as http routes and snapshotting) per agent from the code itself.

## Example

The post demonstrates configuration through an application manifest using three agents: `InboxAgent`, `EscalationAgent`, and `AuditAgent`. Each agent receives distinct environment variables, file paths, and typed configuration objects.

```yaml
manifestVersion: "1.5.0"
app: supportdesk

componentTemplates:
  shared-runtime:
    env:
      RUST_LOG: info
      TZ: UTC
    files:
      - sourcePath: ./shared/ca-certificates.pem
        targetPath: /certs/ca-certificates.pem
        permissions: read-only
    presets:
      local:
        default: true
        env:
          GOLEM_ENV: local
      cloud:
        env:
          GOLEM_ENV: cloud

components:
  supportdesk:agents:
    dir: .
    templates: shared-runtime

agents:
  InboxAgent:
    env:
      OPENAI_API_KEY: "{{ OPENAI_API_KEY }}"
      MODEL: gpt-4.1-mini
    files:
      - sourcePath: ./prompts/inbox-system.md
        targetPath: /prompts/system.md
        permissions: read-only
      - sourcePath: ./data/routing-rules.json
        targetPath: /data/routing-rules.json
        permissions: read-only
    config:
      defaultQueue: general
      summarizeReplies: true
      classification:
        confidenceThreshold: 0.75
        labels:
          - billing
          - outage
          - product

  EscalationAgent:
    env:
      JIRA_BASE_URL: https://acme.atlassian.net
      JIRA_TOKEN: "{{ JIRA_TOKEN }}"
      MODEL: claude-3-7-sonnet
    files:
      - sourcePath: ./prompts/escalation-system.md
        targetPath: /prompts/system.md
        permissions: read-only
      - sourcePath: ./playbooks/p1-outage.md
        targetPath: /playbooks/p1-outage.md
        permissions: read-only
      - sourcePath: https://example.com/runbooks/severity-guide.md
        targetPath: /playbooks/severity-guide.md
        permissions: read-only
    config:
      projectKey: OPS
      defaultPriority: high
      pagerduty:
        serviceId: P123456
        autoPageAfterMinutes: 5

  AuditAgent:
    env:
      S3_BUCKET: supportdesk-audit
    files:
      - sourcePath: ./schemas/audit-event.schema.json
        targetPath: /schemas/audit-event.schema.json
        permissions: read-only
    config:
      redactFields:
        - email
        - phone
        - paymentToken
      retentionDays: 90

bridge:
  ts:
    agents:
      - InboxAgent
      - EscalationAgent
  rust:
    agents: AuditAgent

environments:
  local:
    default: true
    server: local
    componentPresets: local
  prod:
    server: cloud
    componentPresets: cloud
```

Key features shown include:

- **Component templates** applying shared settings across multiple components
- **Per-agent overrides** for environment variables and files
- **Environment presets** (local/cloud) with different configurations
- **Bridge generator configuration** selecting specific agents for code generation
- **Environment definitions** using `componentPresets` for deployment targets

Despite supporting complex setups, the Golem application manifest remains concise by default. Support includes a JSON schema and a dedicated agent skill for manifest editing.
