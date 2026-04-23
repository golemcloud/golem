---
name: golem-cloud-account-setup
description: "Setting up a Golem Cloud account from scratch. Use when creating a Golem Cloud account, authenticating with Golem Cloud via the CLI, setting up a cloud profile, or deploying to Golem Cloud for the first time."
---

# Setting Up a Golem Cloud Account

Both `golem` and `golem-cli` can be used — all commands below work with either binary. The `golem` binary is a superset that includes a built-in local server.

## Overview

Golem Cloud is the hosted version of Golem. To deploy agents to Golem Cloud you need:

1. A **GitHub account** (used for OAuth2 authentication)
2. A **cloud CLI profile** configured in the Golem CLI
3. A **Golem Cloud account** (created automatically on first authentication, or manually)

## Step 1: Create a Cloud Profile

The CLI ships with a built-in `cloud` profile that points to `https://release.api.golem.cloud` with OAuth2 authentication. You can use it directly:

```shell
golem -C profile get          # Show the built-in cloud profile
```

If you need a custom cloud profile (e.g., for a different cloud endpoint):

```shell
golem profile new my-cloud --url https://release.api.golem.cloud --set-active
```

When no `--static-token` is provided, the profile uses OAuth2 (GitHub) authentication — a browser window will open on first use.

## Step 2: Authenticate

Authentication happens automatically the first time you run a command against the cloud profile. The CLI will:

1. Open a GitHub OAuth2 authorization URL in your browser
2. Wait for you to authorize the Golem Cloud application on GitHub
3. Store the resulting token in your profile configuration (`~/.golem/config.json`)

To trigger authentication explicitly:

```shell
golem -C cloud account get    # Triggers OAuth2 flow if not yet authenticated
```

The CLI displays:
```
┌────────────────────────────────────────┐
│       Authenticate with GitHub         │
│                                        │
│  Visit the following URL in a browser  │
│                                        │
└────────────────────────────────────────┘
https://github.com/login/device
──────────────────────────────────────────
Waiting for authentication...
```

Follow the URL, authorize the application, and the CLI will complete automatically.

## Step 3: Manage Your Account

After authentication, you can manage your Golem Cloud account:

```shell
golem -C cloud account get                                          # View account info
golem -C cloud account update "My Name" "me@example.com"            # Update name/email
golem -C cloud account new "Team Account" "team@example.com"        # Create additional account
```

## Step 4: Create and Manage API Tokens

For programmatic access (CI/CD, scripts), create static API tokens:

```shell
golem -C cloud token list                                           # List existing tokens
golem -C cloud token new                                            # Create a new token (default: expires 2100-01-01)
golem -C cloud token new --expires-at 2025-12-31T00:00:00Z          # Create with custom expiry
golem -C cloud token delete <TOKEN_ID>                              # Delete a token
```

Use a static token in a profile for non-interactive environments:

```shell
golem profile new ci-cloud --url https://release.api.golem.cloud --static-token "<TOKEN_SECRET>" --set-active
```

## Step 5: Configure Your Application for Cloud Deployment

In your `golem.yaml`, add a `cloud` environment:

```yaml
environments:
  local:
    default: true
    server: local
    componentPresets: local
  cloud:
    server: cloud
    componentPresets: cloud
```

Then deploy with:

```shell
golem -C deploy               # Deploy to cloud using the cloud environment
```

Or explicitly:

```shell
golem -e cloud deploy         # Deploy using the named "cloud" environment
```

## Using the `-C` and `-L` Shortcuts

| Flag | Effect |
|------|--------|
| `-C` / `--cloud` | Use the `cloud` environment (or `cloud` profile if no manifest) |
| `-L` / `--local` | Use the `local` environment (or `local` profile if no manifest) |

These shortcuts work with any command:

```shell
golem -C component list       # List components on Golem Cloud
golem -C agent list           # List agents on Golem Cloud
golem -L deploy               # Deploy to local server
```

## Golem Cloud Console

Golem Cloud also provides a web management console at [console.golem.cloud](https://console.golem.cloud/) for visual management of components, agents, and deployments.

## Quick Start Summary

```shell
# 1. Authenticate with Golem Cloud (opens browser for GitHub OAuth2)
golem -C cloud account get

# 2. Deploy your application to the cloud
golem -C deploy

# 3. Interact with your cloud agents
golem -C agent list
```

## Related Skills

- Load `golem-profiles-and-environments` for detailed profile, environment, and preset configuration
- Load `golem-deploy` for deployment commands and flags
