//! `install`: copy files into place and create directories, the forms that don't need real
//! permission or ownership changes. WASI has neither (see the README's Limits section), so
//! `-m`/`-o`/`-g` are accepted and ignored rather than refused -- a script that always passes
//! `-m 755` shouldn't have to special-case this sandbox to keep working.
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use brush_core::builtins::{ContentOptions, ContentType, Registration, SimpleCommand};
use brush_core::commands::ExecutionContext;
use brush_core::extensions::ShellExtensions;
use brush_core::{Error, ExecutionResult};

use crate::manifest::Manifest;

pub(crate) struct Install;

impl Install {
    pub(crate) const NAME: &'static str = "install";
    pub(crate) const SYNOPSIS: &'static str = "copy files and set attributes";
}

/// GNU's simple single-quoting: wrap in `'...'`, escaping an embedded `'` as `'\''`.
fn quote_name(name: &str) -> String {
    format!("'{}'", name.replace('\'', "'\\''"))
}

#[derive(Default)]
struct Options {
    /// `-d`/`--directory`: operands are directories to create, not files to copy.
    directory_mode: bool,
    /// `-D`: create the destination's missing leading directories first.
    make_parents: bool,
    /// `-t DIR`/`--target-directory=DIR`.
    target_dir: Option<String>,
    /// `-v`/`--verbose`.
    verbose: bool,
    /// `-b` or an explicit `-S`/`--suffix`: back up an existing destination before replacing it.
    backup: bool,
    /// `-S SUFFIX`/`--suffix=SUFFIX`; GNU's own default is `~`.
    suffix: String,
}

/// Create `path` and every missing leading directory, GNU `install -d`/`-D` style: each
/// directory actually created (not one that already existed) is reported individually when
/// `verbose`, in the order they're created (outermost first).
fn make_directory_tree(path: &str, verbose: bool, out: &mut dyn Write) -> Result<(), (String, u8)> {
    let mut built = PathBuf::new();
    let mut components = Path::new(path).components().peekable();
    while let Some(component) = components.next() {
        built.push(component);
        if built.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(&built) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err((
                    format!(
                        "cannot create directory {}: File exists",
                        quote_name(&built.to_string_lossy())
                    ),
                    1,
                ));
            }
            Err(_) => {
                if let Err(e) = fs::create_dir(&built) {
                    return Err((
                        format!(
                            "cannot create directory {}: {}",
                            quote_name(&built.to_string_lossy()),
                            super::io_message(&e)
                        ),
                        1,
                    ));
                }
                if verbose {
                    let _ = writeln!(
                        out,
                        "install: creating directory {}",
                        quote_name(&built.to_string_lossy())
                    );
                }
            }
        }
        let _ = components.peek();
    }
    Ok(())
}

/// Back up `dst` to `dst<suffix>` before it's overwritten, GNU's simple (non-numbered) backup.
fn backup_existing(dst: &Path, suffix: &str) -> std::io::Result<()> {
    if !dst.exists() {
        return Ok(());
    }
    let backup = PathBuf::from(format!("{}{suffix}", dst.to_string_lossy()));
    match fs::rename(dst, &backup) {
        Ok(()) => Ok(()),
        // A cross-device or otherwise non-renameable destination: copy the bytes across instead.
        Err(_) => {
            fs::copy(dst, &backup)?;
            Ok(())
        }
    }
}

/// Copy one `src` to the literal path `dst` (never a directory to install into -- the caller
/// has already resolved that).
fn install_one(
    src: &str,
    dst: &Path,
    opts: &Options,
    out: &mut dyn Write,
) -> Result<(), (String, u8)> {
    let src_meta = fs::symlink_metadata(src).map_err(|e| {
        (
            format!("cannot stat {}: {}", quote_name(src), super::io_message(&e)),
            1,
        )
    })?;
    if src_meta.is_dir() {
        return Err((format!("omitting directory {}", quote_name(src)), 1));
    }
    if opts.backup {
        // A failed backup is not fatal to the install itself, matching GNU's own leniency here
        // (a backup is best-effort insurance, not the point of the command).
        let _ = backup_existing(dst, &opts.suffix);
    }
    fs::copy(src, dst).map_err(|e| {
        (
            format!(
                "cannot create regular file {}: {}",
                quote_name(&dst.to_string_lossy()),
                super::io_message(&e)
            ),
            1,
        )
    })?;
    if opts.verbose {
        let _ = writeln!(
            out,
            "{} -> {}",
            quote_name(src),
            quote_name(&dst.to_string_lossy())
        );
    }
    Ok(())
}

impl SimpleCommand for Install {
    fn get_content(
        name: &str,
        content_type: ContentType,
        _options: &ContentOptions,
    ) -> Result<String, Error> {
        match content_type {
            ContentType::ShortDescription => Ok(format!("{name} - {}\n", Install::SYNOPSIS)),
            ContentType::ShortUsage => Ok(format!(
                "{name}: {name} [-Dv] [-t DIR] SOURCE DEST\n  or:  {name} [-Dv] [-t DIR] SOURCE... DIRECTORY\n  or:  {name} [-v] -d DIRECTORY...\n"
            )),
            ContentType::DetailedHelp => Ok(format!(
                "{name} - {}\n\nHonest wasm subset: copies file contents and creates directories; \
                 WASI has no permission bits or owners, so -m/-o/-g are accepted and ignored \
                 rather than changing anything (see the README's Limits section).\n",
                Install::SYNOPSIS
            )),
            ContentType::ManPage => brush_core::error::unimp("man page not yet implemented"),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn execute<SE, I, S>(
        context: ExecutionContext<'_, SE>,
        args: I,
    ) -> Result<ExecutionResult, Error>
    where
        SE: ShellExtensions,
        I: Iterator<Item = S>,
        S: AsRef<str>,
    {
        let _cwd = super::coreutils::ShellCwd::enter(&context);
        let argv: Vec<String> = args.skip(1).map(|s| s.as_ref().to_string()).collect();

        let mut opts = Options {
            suffix: "~".to_owned(),
            ..Options::default()
        };
        let mut operands: Vec<String> = Vec::new();
        let mut iter = argv.into_iter();
        let mut options_done = false;
        macro_rules! usage_error {
            ($msg:expr) => {{
                let _ = writeln!(
                    context.stderr(),
                    "install: {}\nTry 'install --help' for more information.",
                    $msg
                );
                return Ok(ExecutionResult::new(1));
            }};
        }
        while let Some(arg) = iter.next() {
            if options_done {
                operands.push(arg);
                continue;
            }
            match arg.as_str() {
                "--" => options_done = true,
                "-d" | "--directory" => opts.directory_mode = true,
                "-D" => opts.make_parents = true,
                "-v" | "--verbose" => opts.verbose = true,
                "-b" => opts.backup = true,
                "-t" | "--target-directory" => match iter.next() {
                    Some(v) => opts.target_dir = Some(v),
                    None => {
                        usage_error!("option '--target-directory' requires an argument")
                    }
                },
                "-S" | "--suffix" => match iter.next() {
                    Some(v) => {
                        opts.suffix = v;
                        opts.backup = true;
                    }
                    None => usage_error!("option '--suffix' requires an argument"),
                },
                "-m" | "--mode" | "-o" | "--owner" | "-g" | "--group" => {
                    // Accepted and ignored: no permission bits or owners exist here (see the
                    // module doc comment). Still consume the value so it isn't mistaken for an
                    // operand.
                    if iter.next().is_none() {
                        usage_error!(format!("option '{arg}' requires an argument"));
                    }
                }
                f if f.starts_with("--target-directory=") => {
                    opts.target_dir = Some(f["--target-directory=".len()..].to_string());
                }
                f if f.starts_with("--suffix=") => {
                    opts.suffix = f["--suffix=".len()..].to_string();
                    opts.backup = true;
                }
                f if f.starts_with("--mode=")
                    || f.starts_with("--owner=")
                    || f.starts_with("--group=") => {}
                f if f.starts_with("--") => {
                    let _ = writeln!(
                        context.stderr(),
                        "install: unrecognized option '{f}'\nTry 'install --help' for more information."
                    );
                    return Ok(ExecutionResult::new(1));
                }
                f if f.starts_with('-') && f.len() > 1 => {
                    // A cluster of the plain boolean short flags (`-Dv`, `-dv`); anything else
                    // packed in with them (`-m`, `-o`, `-g`, `-t`, `-S`, all of which need a
                    // value) isn't supported clustered, only standalone.
                    let mut rejected = None;
                    for c in f[1..].chars() {
                        match c {
                            'd' => opts.directory_mode = true,
                            'D' => opts.make_parents = true,
                            'v' => opts.verbose = true,
                            'b' => opts.backup = true,
                            other => {
                                rejected = Some(other);
                                break;
                            }
                        }
                    }
                    if let Some(other) = rejected {
                        let _ = writeln!(
                            context.stderr(),
                            "install: invalid option -- '{other}'\nTry 'install --help' for more information."
                        );
                        return Ok(ExecutionResult::new(1));
                    }
                }
                op => operands.push(op.to_string()),
            }
        }

        if opts.directory_mode {
            if operands.is_empty() {
                usage_error!("missing operand");
            }
            let mut out = context.stdout();
            let mut failed = false;
            for dir in &operands {
                if let Err((msg, code)) = make_directory_tree(dir, opts.verbose, &mut out) {
                    let _ = writeln!(context.stderr(), "install: {msg}");
                    failed = failed || code != 0;
                }
            }
            return Ok(ExecutionResult::new(u8::from(failed)));
        }

        // Resolve the (source, destination) pairs this call installs.
        let installs: Vec<(String, PathBuf)> = if let Some(dir) = opts.target_dir.clone() {
            if operands.is_empty() {
                usage_error!("missing file operand");
            }
            if opts.make_parents {
                let mut out = context.stdout();
                if let Err((msg, _)) = make_directory_tree(&dir, opts.verbose, &mut out) {
                    drop(out);
                    let _ = writeln!(context.stderr(), "install: {msg}");
                    return Ok(ExecutionResult::new(1));
                }
            } else if !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
                let _ = writeln!(
                    context.stderr(),
                    "install: failed to access {}: No such file or directory",
                    quote_name(&dir)
                );
                return Ok(ExecutionResult::new(1));
            }
            operands
                .iter()
                .map(|src| {
                    let name = Path::new(src)
                        .file_name()
                        .map_or_else(|| src.clone().into(), std::ffi::OsStr::to_os_string);
                    (src.clone(), Path::new(&dir).join(name))
                })
                .collect()
        } else {
            match operands.len() {
                0 => usage_error!("missing file operand"),
                1 => usage_error!(format!(
                    "missing destination file operand after {}",
                    quote_name(&operands[0])
                )),
                2 => {
                    let (src, dst) = (&operands[0], &operands[1]);
                    if !opts.make_parents && fs::symlink_metadata(dst).is_ok_and(|m| m.is_dir()) {
                        let name = Path::new(src)
                            .file_name()
                            .map_or_else(|| src.clone().into(), std::ffi::OsStr::to_os_string);
                        vec![(src.clone(), Path::new(dst).join(name))]
                    } else {
                        if opts.make_parents
                            && let Some(parent) = Path::new(dst).parent()
                            && !parent.as_os_str().is_empty()
                        {
                            let mut out = context.stdout();
                            if let Err((msg, _)) = make_directory_tree(
                                &parent.to_string_lossy(),
                                opts.verbose,
                                &mut out,
                            ) {
                                drop(out);
                                let _ = writeln!(context.stderr(), "install: {msg}");
                                return Ok(ExecutionResult::new(1));
                            }
                        }
                        vec![(src.clone(), PathBuf::from(dst))]
                    }
                }
                _ => {
                    let dir = operands.last().cloned().unwrap();
                    if !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
                        let _ = writeln!(
                            context.stderr(),
                            "install: target {} is not a directory",
                            quote_name(&dir)
                        );
                        return Ok(ExecutionResult::new(1));
                    }
                    operands[..operands.len() - 1]
                        .iter()
                        .map(|src| {
                            let name = Path::new(src)
                                .file_name()
                                .map_or_else(|| src.clone().into(), std::ffi::OsStr::to_os_string);
                            (src.clone(), Path::new(&dir).join(name))
                        })
                        .collect()
                }
            }
        };

        let mut out = context.stdout();
        let mut failed = false;
        for (src, dst) in &installs {
            if let Err((msg, code)) = install_one(src, dst, &opts, &mut out) {
                let _ = writeln!(context.stderr(), "install: {msg}");
                failed = failed || code != 0;
            }
        }
        Ok(ExecutionResult::new(u8::from(failed)))
    }
}

pub(crate) fn builtins<SE: ShellExtensions>() -> Vec<(String, Registration<SE>)> {
    use crate::helpshim::simple_utility_with_help;
    vec![("install".into(), simple_utility_with_help::<Install, SE>())]
}

pub(crate) fn manifests() -> Vec<Manifest> {
    vec![Manifest::builtin(Install::NAME, Install::SYNOPSIS).with_help(
        "install [-Dv] [-t DIR] SOURCE DEST | SOURCE... DIRECTORY; install [-v] -d DIRECTORY... \
         — copy files into place, creating destination directories with -D/-d. -b/-S back up an \
         existing destination. -m/-o/-g are accepted and ignored: WASI has no permission bits \
         or owners for them to change.",
    )]
}
