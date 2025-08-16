# Golem Cloud templates

This repository contains all the *templates* available for the `golem` CLI tool using via the `golem component new` command.

> [!CAUTION]
> **While some templates might work as they are, using them directly is not supported.**

## Structure

The templates are organized to directories per **guest languages**. Each guest language directory contains an `INSTRUCTIONS` text file, which is a template itself and gets printed as a result of the `golem new` command.

Each subdirectory of the guest languages is a template where the directory's name becomes the template's name.

Each **template** consists of arbitrary number of files and subdirectories and a `metadata.json` file.

The `golem new` command applies the below defined **template rules** for each file's and directory's name, and for each file's contents.

The metadata file contains required information and also allows some additional project generation steps to be enabled.

### Metadata JSON
The following fields are required:

- `description` is a free-text description of the template

The following fields are optional:

- `requiresAdapter` is a boolean, defaults to **true**. If true, the appropriate version of the WASI Preview2 to Preview1 adapter is copied into the generated project (based on the guest language) to an `adapters` directory.
- `adapterTarget` is an optional directory path that overrides the default `adapter` directory, when set and `requiresAdapter` is not, then the latter is implicitly set to **true**
- `requiresGolemHostWIT` is a boolean, defaults to **false**. If true, the Golem specific WIT interface gets copied into `wit/deps`.
- `requiresWASI` is a boolean, defaults to **false**. If true, the WASI Preview2 WIT interfaces which are compatible with Golem Cloud get copied into `wit/deps`.
- `witDepsPaths` is an array of directory paths, defaults to **null**. When set, overrides the `wit/deps` directory for the above options and allows to use multiple target dirs for supporting multi-component templates.
- `exclude` is a list of sub-paths and works as a simplified `.gitignore` file. It's primary purpose is to help the development loop of working on templates and in the future it will likely be dropped in favor of just using `.gitignore` files.
- `transformExclude` is an optional list of file names, defaults to **null**. Files with name in this list will not be transformed, only copied.
- `transform` is an optional boolean, defaults to **true**. When set no transformations are applied to any files, useful for common app templates.
- `instructions` is an optional filename, defaults to **null**. When set, overrides the __INSTRUCTIONS__ file used for the template, the file needs to be placed to same directory as the default instructions file.
- `appCommonGroup` is used to mark the template to be part of a composable app template group as a common template
- `appComponentGroup` is used to mark the template to be part of a composable app template group as a component template

### Template rules

Golem templates are currently simple and not using any known template language, in order to keep the templates **compilable** as they are - this makes it very convenient to work on existing ones and add new templates as you can immediately verify that it can be compiled into a _Golem template_.

When calling `golem-new` the user specifies a **template name**. The provided component name must use either `PascalCase`, `snake_case` or `kebab-case`.

There is an optional parameter for defining a **package name**, which defaults to `golem:component`. It has to be in the `pack:name` format. The first part of the package name is called **package namespace**.

The following occurrences get replaced to the provided component name, applying the casing used in the template:
- `componentname` (unchanged)
- `component-name`
- `componentName`
- `ComponentName`
- `component_name`
- `pack::name`
- `pa_ck::na_me` (for rust binding import)
- `pack:name`
- `pack_name`
- `pack-name`
- `pack/name`
- `PackName`
- `pack-ns`  (`pack`)
- `PackNs`   (`Pack`)
- `__pack__` (`pack`)
- `__name__` (`name`)

### Testing the templates
The component generation and instructions can be tested with a test [cli app](/src/test/main.rs).
The app also accepts a filter argument, which matches for the template name as regular expressions, eg. to test the go templates use:

```shell
cargo run --bin golem-templates-test-cli -- templates -f go
```

Or to exactly match a template name:

```shell
cargo run --bin golem-templates-test-cli -- templates -f '^go-default$'
```

The necessary tooling for the specific language is expected to be available.

The test app will instantiate templates and then execute the instructions (all lines starting with `  `).

The components are generated in the `/templates-test` directory.

### Testing the composable app templates

```shell
cargo run --bin golem-templates-test-cli app
```
