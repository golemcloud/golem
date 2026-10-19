//! Render documentation for embedded commands and shell builtins.
use std::io::Write;

use brush_core::builtins::{ContentOptions, ContentType, Registration, SimpleCommand};
use brush_core::commands::ExecutionContext;
use brush_core::extensions::ShellExtensions;
use brush_core::{Error, ExecutionResult};

use crate::manifest::Manifest;

pub(crate) struct Man;

impl Man {
    const NAME: &'static str = "man";
    const SYNOPSIS: &'static str = "display command documentation";
}

/// Render one page from the registry manifest: a minimal NAME/DESCRIPTION layout so the output
/// reads like a man page without pretending to be roff.
fn render_manifest_page(m: &Manifest) -> String {
    format!(
        "{name}(1)\n\nNAME\n    {name} - {synopsis}\n\nDESCRIPTION\n    {help}\n",
        name = m.name,
        synopsis = m.synopsis,
        help = m.help_text.replace('\n', "\n    "),
    )
}

impl SimpleCommand for Man {
    fn get_content(
        name: &str,
        content_type: ContentType,
        _options: &ContentOptions,
    ) -> Result<String, Error> {
        match content_type {
            ContentType::ShortDescription => Ok(format!("{name} - {}\n", Man::SYNOPSIS)),
            ContentType::ShortUsage => Ok(format!("{name}: {name} [section] NAME...\n")),
            ContentType::DetailedHelp => Ok(format!(
                "{name} - {}\n\nRenders documentation from the command inventory or Brush's builtin help.\n",
                Man::SYNOPSIS
            )),
            ContentType::ManPage => brush_core::error::unimp("man page not yet implemented"),
        }
    }

    fn execute<SE, I, S>(
        context: ExecutionContext<'_, SE>,
        args: I,
    ) -> Result<ExecutionResult, Error>
    where
        SE: ShellExtensions,
        I: Iterator<Item = S>,
        S: AsRef<str>,
    {
        let operands: Vec<String> = args
            .skip(1)
            .map(|s| s.as_ref().to_string())
            .filter(|a| !a.starts_with('-'))
            .collect();

        // `man 1 grep` — swallow a leading all-digits section operand when a name follows.
        let names: Vec<&String> =
            if operands.len() > 1 && operands[0].chars().all(|c| c.is_ascii_digit()) {
                operands.iter().skip(1).collect()
            } else {
                operands.iter().collect()
            };

        if names.is_empty() {
            let _ = writeln!(context.stderr(), "What manual page do you want?");
            return Ok(ExecutionResult::new(1));
        }

        let mut out = context.stdout();
        let mut missing = false;
        let registry = crate::registry::build();
        for name in names {
            if let Some(m) = registry.get(name) {
                let _ = write!(out, "{}", render_manifest_page(m));
            } else if let Some(reg) = context.shell.builtins().get(name.as_str()) {
                if let Ok(content) =
                    (reg.content_func)(name, ContentType::DetailedHelp, &ContentOptions::default())
                {
                    let _ = write!(out, "{content}");
                    if !content.ends_with('\n') {
                        let _ = writeln!(out);
                    }
                } else {
                    let _ = writeln!(context.stderr(), "No manual entry for {name}");
                    missing = true;
                }
            } else {
                let _ = writeln!(context.stderr(), "No manual entry for {name}");
                missing = true;
            }
        }
        Ok(ExecutionResult::new(u8::from(missing)))
    }
}

pub(crate) fn builtins<SE: ShellExtensions>() -> Vec<(String, Registration<SE>)> {
    use crate::helpshim::simple_utility_with_help;
    vec![("man".into(), simple_utility_with_help::<Man, SE>())]
}

pub(crate) fn manifests() -> Vec<Manifest> {
    vec![Manifest::builtin(Man::NAME, Man::SYNOPSIS).with_help(
        "man [section] NAME... — display documentation for a command. Rendered from the \
         command inventory, or Brush's builtin help for any \
         other builtin. Section numbers are accepted and ignored.",
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_page_renders_name_and_description() {
        let m = Manifest::builtin("grep", "search files for a pattern");
        let page = render_manifest_page(&m);
        assert!(page.starts_with("grep(1)\n"));
        assert!(page.contains("NAME\n    grep - search files for a pattern"));
        assert!(page.contains("DESCRIPTION"));
    }
}
