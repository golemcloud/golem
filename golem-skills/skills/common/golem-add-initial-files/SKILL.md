---
name: golem-add-initial-files
description: "Adding initial files to Golem agent filesystems. Use when configuring files in golem.yaml at the component template, component, agent type, or preset level, specifying local or remote file sources, setting file permissions, or using merge modes."
---

# Adding Initial Files to Golem Agent Filesystems

Initial files (IFS — Initial File System) provision files into an agent's virtual filesystem when it starts. Files are defined in the application manifest (`golem.yaml`) and uploaded to the Golem registry during `golem deploy`. They follow the same **cascade property system** as environment variables.

## File Entry Structure

Each file entry has three fields:

```yaml
files:
  - sourcePath: ./data/config.json       # local path (relative to golem.yaml) or URL
    targetPath: /etc/app/config.json      # absolute path inside the agent filesystem
    permissions: readOnly                 # optional: readOnly (default) or readWrite
```

- **`sourcePath`** — the source of the file content. Can be:
  - A **relative local path** (resolved relative to the `golem.yaml` file location)
  - An **absolute local path**
  - An **HTTP/HTTPS URL** to download the file from a remote server
  - A **local directory** — all files in the directory are recursively included, preserving the directory structure under `targetPath`
- **`targetPath`** — the absolute path where the file appears inside the agent's WASI filesystem. Must start with `/`.
- **`permissions`** — optional. Either `readOnly` (default) or `readWrite`.

## Cascade Hierarchy (Most General → Most Specific)

```
componentTemplates → components → agents → presets
```

Each level can define `files` (a list of file entries) and `filesMergeMode` (how to combine with the parent level). More specific levels are applied on top of less specific ones.

## 1. Component Template Level

Define shared initial files for all components that use a template:

```yaml
componentTemplates:
  my-template:
    files:
      - sourcePath: ./shared/defaults.conf
        targetPath: /etc/app/defaults.conf
```

All components referencing `my-template` via `templates: [my-template]` inherit these files.

## 2. Component Level

Define or add initial files for a specific component:

```yaml
components:
  my-ns:my-component:
    templates: [my-template]
    files:
      - sourcePath: ./component-data/schema.sql
        targetPath: /data/schema.sql
```

By default, these files are **appended** to any files inherited from the template.

## 3. Agent Type Level

Define initial files for a specific agent type within a component:

```yaml
agents:
  MyAgent:
    files:
      - sourcePath: ./agent-config/settings.toml
        targetPath: /etc/agent/settings.toml
        permissions: readWrite
```

## 4. Preset Level (Component and Agent)

Both components and agents support **presets** that can add or override initial files. Presets are selected at build/deploy time.

### Component preset:

```yaml
components:
  my-ns:my-component:
    files:
      - sourcePath: ./data/base-config.json
        targetPath: /etc/app/config.json
    presets:
      debug:
        default: {}
        files:
          - sourcePath: ./data/debug-config.json
            targetPath: /etc/app/debug.json
```

### Agent preset:

```yaml
agents:
  MyAgent:
    files:
      - sourcePath: ./agent-data/prod.env
        targetPath: /etc/agent/env
    presets:
      debug:
        default: {}
        filesMergeMode: replace
        files:
          - sourcePath: ./agent-data/debug.env
            targetPath: /etc/agent/env
```

## Complete Multi-Level Example

```yaml
componentTemplates:
  shared:
    files:
      - sourcePath: ./shared/ca-certificates.pem
        targetPath: /etc/ssl/certs/ca-certificates.pem

components:
  my-ns:my-component:
    templates: [shared]
    files:
      - sourcePath: ./data/schema.sql
        targetPath: /data/schema.sql
    presets:
      debug:
        default: {}
        files:
          - sourcePath: ./data/debug-seed.sql
            targetPath: /data/seed.sql

agents:
  MyAgent:
    files:
      - sourcePath: ./agent-config/
        targetPath: /etc/agent/config/
        permissions: readWrite
    presets:
      debug:
        default: {}
        files:
          - sourcePath: ./agent-config/debug-overrides.toml
            targetPath: /etc/agent/config/overrides.toml
```

With the `debug` preset, the final resolved files for `MyAgent` would include:

| Target Path | Source | Level |
|---|---|---|
| `/etc/ssl/certs/ca-certificates.pem` | `./shared/ca-certificates.pem` | componentTemplates.shared |
| `/data/schema.sql` | `./data/schema.sql` | components.my-ns:my-component |
| `/data/seed.sql` | `./data/debug-seed.sql` | components.my-ns:my-component.presets.debug |
| `/etc/agent/config/*` | `./agent-config/` (directory) | agents.MyAgent |
| `/etc/agent/config/overrides.toml` | `./agent-config/debug-overrides.toml` | agents.MyAgent.presets.debug |

## Merge Modes

By default, files from child levels are **appended** to the parent list. You can change this per level with `filesMergeMode`:

| Mode | Behavior |
|---|---|
| `append` | **(default)** Add this level's files after the parent's files |
| `prepend` | Add this level's files before the parent's files |
| `replace` | Discard all parent files, use only this level's files |

### Example: Replace all inherited files

```yaml
agents:
  MyAgent:
    filesMergeMode: replace
    files:
      - sourcePath: ./only-this-file.txt
        targetPath: /data/only-this.txt
```

### Example: Prepend files (higher priority positioning)

```yaml
agents:
  MyAgent:
    filesMergeMode: prepend
    files:
      - sourcePath: ./priority-config.json
        targetPath: /etc/priority.json
```

## Source Types

### Local files

Relative paths are resolved from the directory containing `golem.yaml`:

```yaml
files:
  - sourcePath: ./data/config.json
    targetPath: /etc/app/config.json
```

### Local directories

If `sourcePath` points to a directory, all files are included recursively. The directory structure is preserved under `targetPath`:

```yaml
files:
  - sourcePath: ./static-assets/
    targetPath: /var/www/static/
```

For example, if `./static-assets/` contains `index.html` and `css/style.css`, the agent filesystem will have `/var/www/static/index.html` and `/var/www/static/css/style.css`.

### Remote files (HTTP/HTTPS)

Download files from a URL at deploy time:

```yaml
files:
  - sourcePath: https://example.com/data/model.bin
    targetPath: /data/model.bin
```

Remote sources are downloaded and hashed during `golem deploy`. Only single files are supported for remote sources (not directories).

## File Permissions

Each file can be marked as `readOnly` (default) or `readWrite`:

```yaml
files:
  - sourcePath: ./config.json
    targetPath: /etc/app/config.json
    permissions: readOnly           # agent can only read this file

  - sourcePath: ./scratch/workspace
    targetPath: /tmp/workspace
    permissions: readWrite          # agent can read and write this file
```

When a directory source is used, the permission setting applies to all files within that directory.

## Important Notes

- **Target paths must be unique** — if two file entries resolve to the same target path, deployment fails with a validation error.
- Files are content-addressed — identical files across agent types are stored only once in the registry and shared via hardlinks on the executor node.
- Files are downloaded lazily by the worker executor when the agent is first started, then cached locally for subsequent starts.
- There is a configurable disk space limit for cached initial files on each executor node.
- Initial files are **not** passed per agent instance at creation time via `golem agent new`. They are defined at the agent type level in the manifest and applied to all instances of that type.

## Summary of All Methods

| Method | Scope | Where |
|---|---|---|
| `componentTemplates.*.files` | All components using the template | `golem.yaml` |
| `components.*.files` | Single component, all its agents | `golem.yaml` |
| `agents.*.files` | Single agent type | `golem.yaml` |
| `*.presets.*.files` | Component or agent preset | `golem.yaml` |
| `filesMergeMode` | Controls merge at any level | `golem.yaml` |
