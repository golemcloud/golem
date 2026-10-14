//! What a newly started shell process has, so a child command (`bash -c`, `find -exec`, `xargs`)
//! can start from it instead of from a copy of its caller.
use std::collections::HashSet;
use std::sync::OnceLock;

use brush_core::{Shell, ShellExtensions, options::RuntimeOptions};

struct Baseline {
    /// Variables the shell itself maintains (BASH, UID, LINENO, ...), present in any new shell.
    variables: HashSet<String>,
    options: RuntimeOptions,
}

static BASELINE: OnceLock<Baseline> = OnceLock::new();

/// Records a freshly built session's state as what every new shell starts with.
pub(super) fn record<SE: ShellExtensions>(shell: &Shell<SE>) {
    BASELINE.get_or_init(|| Baseline {
        variables: shell.env().iter().map(|(name, _)| name.clone()).collect(),
        options: shell.options().clone(),
    });
}

/// A copy of `shell` as a new shell process started by it would be: its working directory,
/// exported variables and exported functions, but none of its other variables, functions,
/// aliases or directory stack, and the options a new shell starts with. Variables the shell
/// maintains stay; `IFS` and `OPTIND` return to their defaults.
pub(crate) fn fresh_child<SE: ShellExtensions>(shell: &Shell<SE>) -> Shell<SE> {
    let mut child = shell.clone();
    // A new process has no jobs of its own, and does not list its caller's.
    child.jobs_mut().jobs.clear();
    let Some(baseline) = BASELINE.get() else {
        return child;
    };
    // The environment carries only exported variables with a value, and no arrays.
    let dropped: Vec<String> = child
        .env()
        .iter()
        .filter(|(name, var)| {
            (!var.is_exported() || !var.value().is_set() || var.value().is_array())
                && !baseline.variables.contains(name.as_str())
        })
        .map(|(name, _)| name.clone())
        .collect();
    for name in dropped {
        let _ = child.env_mut().unset_raw(&name);
    }
    for (name, value) in [("IFS", " \t\n"), ("OPTIND", "1")] {
        let _ = child.env_mut().set_global(
            name,
            brush_core::ShellVariable::new(brush_core::ShellValue::String(value.to_owned())),
        );
    }
    let functions: Vec<String> = child
        .funcs()
        .iter()
        .filter(|(_, registration)| !registration.is_exported())
        .map(|(name, _)| name.clone())
        .collect();
    for name in functions {
        child.undefine_func(&name);
    }
    // An exported function reaches a new process through the environment, as bash exports it:
    // the child reads it back from that text, so its source is `environment` and its lines are
    // numbered from 0.
    let exported: Vec<(String, String)> = child
        .funcs()
        .iter()
        .map(|(name, registration)| {
            (
                name.clone(),
                crate::tools::coreutils::exported_function_text(
                    &registration.definition().to_string(),
                ),
            )
        })
        .collect();
    for (name, text) in exported {
        if child.define_func_from_str(name.as_str(), &text).is_ok()
            && let Some(function) = child.func_mut(&name)
        {
            function.export();
        }
    }
    // An exported nameref reaches a new process as an ordinary variable holding the name it
    // refers to: the environment carries no attributes.
    let namerefs: Vec<(String, String)> = child
        .env()
        .iter()
        .filter(|(_, var)| var.is_exported() && var.is_treated_as_nameref())
        .filter_map(|(name, var)| match var.value() {
            brush_core::ShellValue::String(target) => Some((name.clone(), target.clone())),
            _ => None,
        })
        .collect();
    for (name, target) in namerefs {
        let _ = child.env_mut().unset_raw(&name);
        let mut var = brush_core::ShellVariable::new(brush_core::ShellValue::String(target));
        var.export();
        let _ = child.env_mut().set_global(name, var);
    }
    child.aliases_mut().clear();
    child.directory_stack_mut().clear();
    *child.options_mut() = baseline.options.clone();
    child
}
