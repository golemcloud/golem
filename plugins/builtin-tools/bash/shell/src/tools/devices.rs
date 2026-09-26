//! A virtual `/dev`, and virtual standard streams, for in-process commands.
//!
//! WASI has no device files, yet scripts name `/dev/null`, `/dev/stdin`, `/dev/stdout`,
//! `/dev/stderr` and `/dev/fd/N` as operands. Every path a command opens and every descriptor it
//! reads or writes reaches the host through wasi-libc, so the build wraps libc's path and
//! descriptor entry points (`--wrap`, see `build.rs`) and this module answers before they get
//! there:
//!
//! - A command that runs synchronously in this process (the embedded utilities) is *served*
//!   ([`serve`]): its descriptors 0–2 read and write the shell streams it was given, so nothing is
//!   renumbered and no capture file is created.
//! - `/dev/null` opens as a descriptor that reads nothing and keeps nothing, and stats as the
//!   character device 1,3.
//! - `/dev/stdin`, `/dev/stdout`, `/dev/stderr` and `/dev/fd/N` open onto the served command's
//!   descriptors: a pipe or buffer is shared, position and all, as on Linux; a regular file is
//!   opened again with a position of its own, as Linux reopens `/proc/self/fd/N`. With no command
//!   served, the standard streams fail with `ENXIO`; commands that stream through the shell's own
//!   pipes map these paths themselves with [`classify`], and open a regular file again with
//!   [`reopen`].
//! - `/dev` and `/dev/fd` stat as directories that cannot be listed.
use std::path::Path;

/// A device path, by what it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Device {
    Directory,
    Null,
    /// A standard stream: 0, 1 or 2.
    Stream(usize),
    /// Another of the command's descriptors, such as a process substitution's `/dev/fd/63`.
    Descriptor(i32),
}

/// A device path and, when the path itself is a symbolic link (as `/dev/stdin` is on Linux), the
/// link's target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    device: Device,
    link: Option<&'static str>,
    /// The path asks for a directory (a trailing `/`) that the device is not: `ENOTDIR`.
    not_dir: bool,
}

/// The device an absolute path names, if any. A path such as `/dev/null/`, which names none
/// because the device is no directory, is left to the file system, where it fails.
pub(crate) fn classify(path: &Path) -> Option<Device> {
    lookup(path.as_os_str().as_encoded_bytes())
        .filter(|entry| !entry.not_dir)
        .map(|entry| entry.device)
}

/// Whether `path` is inside the virtual `/dev` without naming a device (`/dev/foo`): nothing can
/// be created or removed there, as for a user in a Linux `/dev`.
#[cfg_attr(
    not(target_arch = "wasm32"),
    allow(dead_code, reason = "only wasm32 serves the virtual /dev")
)]
fn inside_dev(path: &[u8]) -> bool {
    path.first() == Some(&b'/')
        && lookup(path).is_none()
        && normalize(path).first() == Some(&&b"dev"[..])
}

fn normalize(path: &[u8]) -> Vec<&[u8]> {
    let mut parts: Vec<&[u8]> = Vec::new();
    for part in path.split(|&byte| byte == b'/') {
        match part {
            b"" | b"." => {}
            b".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts
}

/// The device a command operand names: `-` is standard input, and a relative path resolves
/// against the process's working directory, which commands set to the shell's.
pub(crate) fn operand(operand: &str) -> Option<Device> {
    if operand == "-" {
        return Some(Device::Stream(0));
    }
    classify(&std::path::absolute(operand).ok()?)
}

fn lookup(path: &[u8]) -> Option<Entry> {
    const STREAM_LINKS: [&str; 3] = ["/proc/self/fd/0", "/proc/self/fd/1", "/proc/self/fd/2"];
    // Almost every path is not a device; reject those before normalising.
    if path.first() != Some(&b'/') || !(contains(path, b"dev") || contains(path, b"proc")) {
        return None;
    }
    let parts = normalize(path);
    let wants_directory = path.ends_with(b"/") || path.ends_with(b"/.") || path.ends_with(b"/..");
    let descriptor = |name: &[u8]| {
        let fd: i32 = std::str::from_utf8(name).ok()?.parse().ok()?;
        match fd {
            0..=2 => usize::try_from(fd).ok().map(Device::Stream),
            _ => Some(Device::Descriptor(fd)),
        }
    };
    let entry = |device, link| {
        Some(Entry {
            device,
            link,
            not_dir: wants_directory && device != Device::Directory,
        })
    };
    // A path that goes on past a device that is no directory (`/dev/null/x`) fails with
    // ENOTDIR, as on Linux.
    let beyond = |device| {
        Some(Entry {
            device,
            link: None,
            not_dir: true,
        })
    };
    match parts.as_slice() {
        [b"dev", b"null", _, ..] => beyond(Device::Null),
        [b"dev", b"stdin", _, ..] => beyond(Device::Stream(0)),
        [b"dev", b"stdout", _, ..] => beyond(Device::Stream(1)),
        [b"dev", b"stderr", _, ..] => beyond(Device::Stream(2)),
        [b"dev" | b"proc", ..] if descriptor_beyond(&parts, descriptor).is_some() => {
            beyond(descriptor_beyond(&parts, descriptor)?)
        }
        [b"dev"] => entry(Device::Directory, None),
        [b"dev", b"null"] => entry(Device::Null, None),
        [b"dev", b"stdin"] => entry(Device::Stream(0), Some(STREAM_LINKS[0])),
        [b"dev", b"stdout"] => entry(Device::Stream(1), Some(STREAM_LINKS[1])),
        [b"dev", b"stderr"] => entry(Device::Stream(2), Some(STREAM_LINKS[2])),
        [b"dev", b"fd"] => entry(Device::Directory, Some("/proc/self/fd")),
        // `/dev/fd` is a symlink to `/proc/self/fd` (see above); the target itself must also
        // answer as a directory, or `cd /proc/self/fd` fails while `cd /dev/fd` succeeds.
        [b"proc", b"self", b"fd"] => entry(Device::Directory, None),
        [b"dev", b"fd", name] | [b"proc", b"self", b"fd", name] => entry(descriptor(name)?, None),
        _ => None,
    }
}

/// The descriptor device a path such as `/dev/fd/1/x` goes on past, if it does.
fn descriptor_beyond(
    parts: &[&[u8]],
    descriptor: impl Fn(&[u8]) -> Option<Device>,
) -> Option<Device> {
    match parts {
        [b"dev", b"fd", name, _, ..] | [b"proc", b"self", b"fd", name, _, ..] => descriptor(name),
        _ => None,
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(target_arch = "wasm32")]
pub(crate) use wasm::{discard_served, reopen, reopen_as, serve, serving, write_to_real_stderr};

#[cfg(target_arch = "wasm32")]
#[allow(
    unsafe_code,
    reason = "libc entry points wrapped at link time; each checks its pointers as libc would"
)]
mod wasm {
    use super::{Device, Entry, inside_dev, lookup};
    use brush_core::openfiles::OpenFile;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::ffi::{CStr, c_char, c_int, c_void};
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd};

    /// The descriptors of a command running synchronously in this process: 0–2 always, and those
    /// above 2 it may name as `/dev/fd/N`. A closed standard descriptor is left out.
    struct Served {
        files: HashMap<i32, OpenFile>,
        /// Standard input is a prefix of what was piped: it stopped at the input limit.
        input_cut: bool,
        /// The command read standard input up to that cut.
        input_cut_reached: bool,
    }

    /// What a descriptor this module handed out refers to.
    #[derive(Clone, Copy)]
    enum Alias {
        /// A served descriptor (`level` in the stack of served commands), sharing its position,
        /// as a pipe's `/dev/fd/N` does.
        Shared { level: usize, fd: i32 },
        /// The regular file behind a served descriptor, opened again as Linux opens `/dev/fd/N`:
        /// the same file with a position of its own.
        Reopened {
            fd: c_int,
            offset: i64,
            append: bool,
        },
        /// The null device.
        Null,
    }

    thread_local! {
        /// The commands being served, innermost last. Commands nest: `run_uu` inside a capture.
        static SERVED: RefCell<Vec<Served>> = const { RefCell::new(Vec::new()) };
        /// Descriptors opened on device paths, by number.
        static ALIASES: RefCell<HashMap<c_int, Alias>> = RefCell::new(HashMap::new());
        /// The next alias number: far above anything libc allocates.
        static NEXT_ALIAS: Cell<c_int> = const { Cell::new(1 << 30) };
        /// The shell's files behind the descriptors [`reopen`] handed out, kept open until those
        /// close.
        static REOPENED: RefCell<HashMap<c_int, std::sync::Arc<std::fs::File>>> =
            RefCell::new(HashMap::new());
    }

    /// Serves a command's descriptors to the libc calls it makes until the guard drops: its
    /// standard streams are `files[0..=2]`, and `/dev/stdin`, `/dev/fd/N` and the like open onto
    /// `files`. No file is created for any of them.
    pub(crate) fn serve(files: HashMap<i32, OpenFile>, input_cut: bool) -> Serving {
        let level = SERVED.with(|served| {
            let mut served = served.borrow_mut();
            served.push(Served {
                files,
                input_cut,
                input_cut_reached: false,
            });
            served.len()
        });
        Serving { level }
    }

    pub(crate) struct Serving {
        level: usize,
    }

    /// Makes the innermost served command's descriptor `fd` discard what is written to it: what a
    /// utility still has buffered for a stream that refused it must not reach the next one.
    pub(crate) fn discard_served(fd: i32) {
        SERVED.with(|served| {
            if let Some(served) = served.borrow_mut().last_mut() {
                served.files.insert(fd, brush_core::openfiles::null_sink());
            }
        });
    }

    /// Whether a command's descriptors are being served.
    pub(crate) fn serving() -> bool {
        SERVED.with(|served| !served.borrow().is_empty())
    }

    /// Writes directly to the real, unwrapped fd 2, bypassing whatever `OpenFile` a served
    /// command's own fd 2 currently answers to (see `Served`/`target`). Used only by the panic
    /// hook `coreutils::install_panic_hook` installs: this target's panic strategy is `abort`, so
    /// a panic inside a served utility call never unwinds back to a caller that could restore
    /// process state and replay the capture -- the hook has to reach the real stream directly,
    /// synchronously, before the trap tears the whole component down.
    pub(crate) fn write_to_real_stderr(bytes: &[u8]) {
        // SAFETY: `__real_write` is the unwrapped `write` syscall entry point (declared below,
        // with the module's other `__real_*` imports); `bytes` is a valid pointer for
        // `bytes.len()` readable bytes for the call's duration. The return value (bytes written,
        // or -1 on error) is intentionally ignored: there is nothing more to do with a panic
        // message that fails to write.
        unsafe {
            __real_write(2, bytes.as_ptr().cast(), bytes.len());
        }
    }

    impl Serving {
        /// Whether the command read its standard input up to the input limit's cut.
        pub(crate) fn input_cut_reached(&self) -> bool {
            SERVED.with(|served| {
                served
                    .borrow()
                    .get(self.level - 1)
                    .is_some_and(|served| served.input_cut_reached)
            })
        }
    }

    impl Drop for Serving {
        fn drop(&mut self) {
            SERVED.with(|served| served.borrow_mut().truncate(self.level - 1));
            let level = self.level;
            ALIASES.with(|aliases| {
                aliases.borrow_mut().retain(
                    |_, alias| !matches!(alias, Alias::Shared { level: at, .. } if *at >= level),
                );
            });
        }
    }

    /// Where a descriptor's calls go when this module answers them.
    #[derive(Clone, Copy)]
    enum Target {
        /// A served descriptor.
        Served {
            level: usize,
            fd: i32,
        },
        Alias(c_int, Alias),
    }

    fn target(fd: c_int) -> Option<Target> {
        if let Some(alias) = ALIASES.with(|aliases| aliases.borrow().get(&fd).copied()) {
            return Some(match alias {
                Alias::Shared { level, fd } => Target::Served { level, fd },
                alias => Target::Alias(fd, alias),
            });
        }
        if !(0..=2).contains(&fd) {
            return None;
        }
        let level = SERVED.with(|served| served.borrow().len());
        (level > 0).then_some(Target::Served { level, fd })
    }

    /// The served descriptor's file: a clone sharing its handle and position, taken out so no
    /// borrow is held while it is used (its own I/O may come back through these wrappers).
    fn served_file(level: usize, fd: i32) -> Option<OpenFile> {
        SERVED.with(|served| {
            served
                .borrow()
                .get(level.checked_sub(1)?)
                .and_then(|served| served.files.get(&fd).cloned())
        })
    }

    fn errno(error: &io::Error) -> c_int {
        if let Some(code) = error.raw_os_error() {
            return code;
        }
        match error.kind() {
            io::ErrorKind::BrokenPipe => libc::EPIPE,
            io::ErrorKind::WouldBlock => libc::EAGAIN,
            io::ErrorKind::NotFound => libc::ENOENT,
            io::ErrorKind::PermissionDenied => libc::EBADF,
            io::ErrorKind::InvalidInput => libc::EINVAL,
            // A process substitution's output cut at the limit (see Brush's `from_bytes_then`).
            io::ErrorKind::Other => libc::EFBIG,
            _ => libc::EIO,
        }
    }

    fn fail(errno: c_int) -> c_int {
        // SAFETY: `__errno_location` returns this thread's errno, always valid to write.
        unsafe { *libc::__errno_location() = errno };
        -1
    }

    fn fail_size(errno: c_int) -> isize {
        fail(errno) as isize
    }

    fn answer(result: Result<c_int, c_int>) -> c_int {
        result.unwrap_or_else(fail)
    }

    fn answer_size(result: Result<usize, c_int>) -> isize {
        result.map_or_else(fail_size, |count| {
            isize::try_from(count).unwrap_or(isize::MAX)
        })
    }

    fn read_target(target: Target, buffer: &mut [u8]) -> Result<usize, c_int> {
        match target {
            Target::Served { level, fd } => {
                let mut file = served_file(level, fd).ok_or(libc::EBADF)?;
                let count = loop {
                    match file.read(buffer) {
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        result => break result.map_err(|error| errno(&error))?,
                    }
                };
                if count == 0 && fd == 0 && !buffer.is_empty() {
                    SERVED.with(|served| {
                        if let Some(served) = served.borrow_mut().get_mut(level - 1)
                            && served.input_cut
                        {
                            served.input_cut_reached = true;
                        }
                    });
                }
                Ok(count)
            }
            Target::Alias(alias_fd, Alias::Reopened { fd, offset, append }) => {
                // SAFETY: `buffer` is valid for `buffer.len()` writable bytes.
                let count =
                    unsafe { libc::pread(fd, buffer.as_mut_ptr().cast(), buffer.len(), offset) };
                let count = usize::try_from(count).map_err(|_| last_errno())?;
                set_offset(alias_fd, fd, offset + count as i64, append);
                Ok(count)
            }
            Target::Alias(_, _) => Ok(0),
        }
    }

    fn write_target(target: Target, buffer: &[u8]) -> Result<usize, c_int> {
        match target {
            Target::Served { level, fd } => {
                let mut file = served_file(level, fd).ok_or(libc::EBADF)?;
                loop {
                    match file.write(buffer) {
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        result => break result.map_err(|error| errno(&error)),
                    }
                }
            }
            Target::Alias(alias_fd, Alias::Reopened { fd, offset, append }) => {
                let offset = if append { file_size(fd)? } else { offset };
                // SAFETY: `buffer` is valid for `buffer.len()` readable bytes.
                let count =
                    unsafe { libc::pwrite(fd, buffer.as_ptr().cast(), buffer.len(), offset) };
                let count = usize::try_from(count).map_err(|_| last_errno())?;
                set_offset(alias_fd, fd, offset + count as i64, append);
                Ok(count)
            }
            Target::Alias(_, _) => Ok(buffer.len()),
        }
    }

    fn set_offset(alias_fd: c_int, fd: c_int, offset: i64, append: bool) {
        ALIASES.with(|aliases| {
            aliases
                .borrow_mut()
                .insert(alias_fd, Alias::Reopened { fd, offset, append });
        });
    }

    fn last_errno() -> c_int {
        io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO)
    }

    fn file_size(fd: c_int) -> Result<i64, c_int> {
        // SAFETY: an all-zero `stat` is a valid value of this plain C struct.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `stat` is writable storage for one `stat`.
        if unsafe { __real_fstat(fd, &raw mut stat) } != 0 {
            return Err(last_errno());
        }
        Ok(stat.st_size)
    }

    /// The raw descriptor of a served descriptor's regular file, if it is one.
    fn served_raw_fd(level: usize, fd: i32) -> Option<c_int> {
        match served_file(level, fd)? {
            OpenFile::File(file) => Some(file.as_raw_fd()),
            _ => None,
        }
    }

    fn open_device(entry: Entry, flags: c_int) -> Result<c_int, c_int> {
        let writes = flags & libc::O_ACCMODE != libc::O_RDONLY;
        if entry.not_dir {
            return Err(libc::ENOTDIR);
        }
        let alias = match entry.device {
            Device::Directory if writes => return Err(libc::EISDIR),
            Device::Directory => return Err(libc::EACCES),
            _ if flags & libc::O_DIRECTORY != 0 => return Err(libc::ENOTDIR),
            Device::Null => Alias::Null,
            device => {
                let fd = device_fd(device).ok_or(libc::EBADF)?;
                let level = SERVED.with(|served| served.borrow().len());
                match served_file(level, fd) {
                    // With no command served, the standard streams are not this process's.
                    None if level == 0 && (0..=2).contains(&fd) => return Err(libc::ENXIO),
                    None => return Err(libc::ENOENT),
                    Some(OpenFile::File(file)) => {
                        let fd = file.as_raw_fd();
                        if writes && flags & libc::O_TRUNC != 0 {
                            // SAFETY: `ftruncate` takes integers only.
                            if unsafe { libc::ftruncate(fd, 0) } != 0 {
                                return Err(last_errno());
                            }
                        }
                        Alias::Reopened {
                            fd,
                            offset: 0,
                            append: flags & libc::O_APPEND != 0,
                        }
                    }
                    Some(_) => Alias::Shared { level, fd },
                }
            }
        };
        Ok(new_alias(alias))
    }

    /// A new descriptor number that answers as `alias`.
    fn new_alias(alias: Alias) -> c_int {
        let fd = NEXT_ALIAS.with(|next| {
            let fd = next.get();
            next.set(fd + 1);
            fd
        });
        ALIASES.with(|aliases| aliases.borrow_mut().insert(fd, alias));
        fd
    }

    /// Opens the regular file behind one of the shell's descriptors again, as Linux opens
    /// `/dev/stdin` or `/dev/fd/N`: the same file with a position of its own from its start, so
    /// reading it leaves the shell's descriptor where it was. For commands that map those paths
    /// onto the shell's descriptors themselves; `file` stays open until the new file closes.
    pub(crate) fn reopen(file: std::sync::Arc<std::fs::File>) -> std::fs::File {
        let fd = new_alias(Alias::Reopened {
            fd: file.as_raw_fd(),
            offset: 0,
            append: false,
        });
        REOPENED.with(|reopened| reopened.borrow_mut().insert(fd, file));
        // SAFETY: every call a `File` makes on `fd` reaches this module's wrappers, which answer
        // for the alias; closing it only forgets it.
        unsafe { std::fs::File::from_raw_fd(fd) }
    }

    /// Opens the regular file behind one of the shell's descriptors again for a redirection to
    /// a name for it (`> /dev/stdout`), as Linux does: with a position of its own, emptied first
    /// for `>`, writing at its end for `>>`.
    pub(crate) fn reopen_as(
        file: std::sync::Arc<std::fs::File>,
        mode: brush_core::openfiles::Reopen,
    ) -> io::Result<std::fs::File> {
        use brush_core::openfiles::Reopen;
        if let Reopen::Write { truncate: true, .. } = mode {
            file.set_len(0)?;
        }
        let fd = new_alias(Alias::Reopened {
            fd: file.as_raw_fd(),
            offset: 0,
            append: matches!(mode, Reopen::Write { append: true, .. }),
        });
        REOPENED.with(|reopened| reopened.borrow_mut().insert(fd, file));
        // SAFETY: as in `reopen`.
        Ok(unsafe { std::fs::File::from_raw_fd(fd) })
    }

    /// The descriptor a stream or descriptor device names.
    fn device_fd(device: Device) -> Option<i32> {
        match device {
            Device::Stream(fd) => i32::try_from(fd).ok(),
            Device::Descriptor(fd) => Some(fd),
            Device::Directory | Device::Null => None,
        }
    }

    fn stat_entry(entry: Entry, follow: bool, buffer: *mut libc::stat) -> c_int {
        if entry.not_dir {
            return fail(libc::ENOTDIR);
        }
        // A closed descriptor of a served command does not exist, as on Linux.
        if let Some(fd) = device_fd(entry.device) {
            let level = SERVED.with(|served| served.borrow().len());
            if level > 0 && served_file(level, fd).is_none() {
                return fail(libc::ENOENT);
            }
            if level > 0
                && follow
                && let Some(raw) = served_raw_fd(level, fd)
            {
                // SAFETY: libc's contract for `fstat`, forwarded.
                return unsafe { __real_fstat(raw, buffer) };
            }
        }
        stat_device(entry, follow, buffer);
        0
    }

    fn stat_device(entry: Entry, follow: bool, buffer: *mut libc::stat) {
        // SAFETY: an all-zero `stat` is a valid value of this plain C struct.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        stat.st_nlink = 1;
        match (entry.link, entry.device) {
            (Some(target), _) if !follow => {
                stat.st_mode = libc::S_IFLNK | 0o777;
                stat.st_size = i64::try_from(target.len()).unwrap_or_default();
            }
            (_, Device::Directory) => {
                stat.st_mode = libc::S_IFDIR | 0o755;
                stat.st_nlink = 2;
            }
            (_, Device::Null) => null_stat(&mut stat),
            (_, Device::Stream(_) | Device::Descriptor(_)) => stat.st_mode = libc::S_IFIFO | 0o600,
        }
        // SAFETY: libc's contract for `stat`: `buffer` points to writable storage for a `stat`.
        unsafe { buffer.write(stat) };
    }

    fn null_stat(stat: &mut libc::stat) {
        stat.st_mode = libc::S_IFCHR | 0o666;
        stat.st_rdev = (1 << 8) | 3;
    }

    /// Whether `path` is the empty string, which names no file on Linux; WASI resolves it to the
    /// working directory, so `ls ''` would list it.
    ///
    /// # Safety
    /// `path` is null or a NUL-terminated string, as libc requires of its callers.
    unsafe fn empty(path: *const c_char) -> bool {
        // SAFETY: the caller's contract: a readable NUL-terminated string when not null.
        !path.is_null() && unsafe { *path } == 0
    }

    /// A failed read of a directory, as Linux reports it: WASI says `EBADF` where Linux says
    /// `EISDIR`.
    fn directory_read_error(fd: c_int) {
        // SAFETY: `__errno_location` returns this thread's errno, always valid to read.
        if unsafe { *libc::__errno_location() } != libc::EBADF {
            return;
        }
        // SAFETY: an all-zero `stat` is a valid value of this plain C struct.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `stat` is writable storage for one `stat`.
        if unsafe { __real_fstat(fd, &raw mut stat) } == 0
            && stat.st_mode & libc::S_IFMT == libc::S_IFDIR
        {
            fail(libc::EISDIR);
        }
    }

    /// The device `path` names; `None` for every other path and for a null pointer.
    ///
    /// # Safety
    /// `path` is null or a NUL-terminated string, as libc requires of its callers.
    unsafe fn entry(path: *const c_char) -> Option<Entry> {
        // SAFETY: the caller's contract, forwarded.
        lookup(&unsafe { absolute(libc::AT_FDCWD, path) }?)
    }

    /// `path` as an absolute path: relative ones resolve against the working directory when
    /// `directory` is it and that is inside `/dev` or `/proc` (where `cd /dev` leaves it); other
    /// relative paths are not devices and stay `None`.
    ///
    /// # Safety
    /// `path` is null or a NUL-terminated string, as libc requires of its callers.
    unsafe fn absolute(directory: c_int, path: *const c_char) -> Option<Vec<u8>> {
        if path.is_null() {
            return None;
        }
        // SAFETY: the caller passes libc's own argument, a NUL-terminated string.
        let path = unsafe { CStr::from_ptr(path) }.to_bytes();
        if path.first() == Some(&b'/') {
            return Some(path.to_vec());
        }
        if directory != libc::AT_FDCWD {
            return None;
        }
        let cwd = std::env::current_dir().ok()?;
        let cwd = cwd.as_os_str().as_encoded_bytes();
        if !(cwd.starts_with(b"/dev") || cwd.starts_with(b"/proc")) {
            return None;
        }
        let mut joined = cwd.to_vec();
        joined.push(b'/');
        joined.extend_from_slice(path);
        Some(joined)
    }

    /// Why a change to the file system at `path` fails, if `path` is a device or inside `/dev`:
    /// what a Linux user gets there.
    ///
    /// # Safety
    /// `path` is null or a NUL-terminated string, as libc requires of its callers.
    unsafe fn refused_change(
        directory: c_int,
        path: *const c_char,
        device_error: c_int,
    ) -> Option<c_int> {
        // SAFETY: the caller's contract, forwarded.
        let path = unsafe { absolute(directory, path) }?;
        match lookup(&path) {
            Some(entry) if entry.not_dir => Some(libc::ENOTDIR),
            Some(_) => Some(device_error),
            None if inside_dev(&path) => Some(libc::EACCES),
            None => None,
        }
    }

    /// Setting the times of an open device: to "now" (which needs only write permission)
    /// succeeds, to given times (which needs ownership, as `cp -p` asks) is not permitted.
    ///
    /// # Safety
    /// `times` is null or points to two `timespec`s, as libc requires.
    unsafe fn device_times(times: *const libc::timespec) -> c_int {
        let now = times.is_null() || {
            // SAFETY: the caller's contract: two readable `timespec`s.
            let times = unsafe { std::slice::from_raw_parts(times, 2) };
            times.iter().all(|time| time.tv_nsec == libc::UTIME_NOW)
        };
        if now { 0 } else { fail(libc::EPERM) }
    }

    fn access_device(entry: Entry, mode: c_int) -> c_int {
        if entry.not_dir {
            fail(libc::ENOTDIR)
        } else if mode & libc::X_OK != 0 && entry.device != Device::Directory {
            fail(libc::EACCES)
        } else {
            0
        }
    }

    fn readlink_device(entry: Entry, buffer: *mut c_char, size: usize) -> isize {
        let Some(target) = entry.link else {
            return isize::try_from(fail(libc::EINVAL)).unwrap_or(-1);
        };
        let length = target.len().min(size);
        // SAFETY: libc's contract for `readlink`: `buffer` has room for `size` bytes, and at most
        // `length <= size` are written. Link targets are static strings, never overlapping it.
        unsafe { std::ptr::copy_nonoverlapping(target.as_ptr().cast(), buffer, length) };
        isize::try_from(length).unwrap_or(isize::MAX)
    }

    // wasi-libc ignores `open`'s mode, so the wrappers take the variadic argument pointer and
    // pass a fixed mode on. Calling through `__real_*` (rather than wasi-libc's internals) makes a
    // build without the `--wrap` flags fail to link instead of silently skipping this module.
    unsafe extern "C" {
        fn __real_open(path: *const c_char, flags: c_int, ...) -> c_int;
        fn __real_openat(directory: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
        fn __real_stat(path: *const c_char, buffer: *mut libc::stat) -> c_int;
        fn __real_lstat(path: *const c_char, buffer: *mut libc::stat) -> c_int;
        fn __real_fstatat(
            directory: c_int,
            path: *const c_char,
            buffer: *mut libc::stat,
            flags: c_int,
        ) -> c_int;
        fn __real_access(path: *const c_char, mode: c_int) -> c_int;
        fn __real_faccessat(
            directory: c_int,
            path: *const c_char,
            mode: c_int,
            flags: c_int,
        ) -> c_int;
        fn __real_readlink(path: *const c_char, buffer: *mut c_char, size: usize) -> isize;
        fn __real_readlinkat(
            directory: c_int,
            path: *const c_char,
            buffer: *mut c_char,
            size: usize,
        ) -> isize;
        fn __real_read(fd: c_int, buffer: *mut c_void, count: usize) -> isize;
        fn __real_readv(fd: c_int, iov: *const libc::iovec, count: c_int) -> isize;
        fn __real_write(fd: c_int, buffer: *const c_void, count: usize) -> isize;
        fn __real_writev(fd: c_int, iov: *const libc::iovec, count: c_int) -> isize;
        fn __real_close(fd: c_int) -> c_int;
        fn __real_lseek(fd: c_int, offset: i64, whence: c_int) -> i64;
        fn __real_fstat(fd: c_int, buffer: *mut libc::stat) -> c_int;
        fn __real_isatty(fd: c_int) -> c_int;
        fn __real_ftruncate(fd: c_int, length: i64) -> c_int;
        fn __real_utimensat(
            directory: c_int,
            path: *const c_char,
            times: *const libc::timespec,
            flags: c_int,
        ) -> c_int;
        fn __real_futimens(fd: c_int, times: *const libc::timespec) -> c_int;
        fn __real_unlink(path: *const c_char) -> c_int;
        fn __real_unlinkat(directory: c_int, path: *const c_char, flags: c_int) -> c_int;
        fn __real_rmdir(path: *const c_char) -> c_int;
        fn __real_mkdir(path: *const c_char, mode: libc::mode_t) -> c_int;
        fn __real_linkat(
            old_directory: c_int,
            old: *const c_char,
            new_directory: c_int,
            new: *const c_char,
            flags: c_int,
        ) -> c_int;
        fn __real_rename(old: *const c_char, new: *const c_char) -> c_int;
        fn __real_symlink(target: *const c_char, path: *const c_char) -> c_int;
        fn __real_opendir(path: *const c_char) -> *mut libc::DIR;
    }

    /// # Safety
    /// libc's contract for `open`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_open(
        path: *const c_char,
        flags: c_int,
        _mode: *const c_void,
    ) -> c_int {
        // SAFETY: libc's contract, forwarded.
        if unsafe { empty(path) } {
            return fail(libc::ENOENT);
        }
        // SAFETY: libc's contract, forwarded.
        match unsafe { entry(path) } {
            Some(entry) => answer(open_device(entry, flags)),
            // SAFETY: libc's contract, forwarded.
            None if flags & libc::O_CREAT != 0
                && unsafe { refused_change(libc::AT_FDCWD, path, libc::EACCES) }.is_some() =>
            {
                fail(libc::EACCES)
            }
            None => unsafe { __real_open(path, flags, 0o666) },
        }
    }

    /// # Safety
    /// libc's contract for `openat`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_openat(
        directory: c_int,
        path: *const c_char,
        flags: c_int,
        _mode: *const c_void,
    ) -> c_int {
        // SAFETY: libc's contract, forwarded.
        if unsafe { empty(path) } {
            return fail(libc::ENOENT);
        }
        // SAFETY: libc's contract, forwarded.
        match unsafe { absolute(directory, path) }.and_then(|path| lookup(&path)) {
            Some(entry) => answer(open_device(entry, flags)),
            // SAFETY: libc's contract, forwarded.
            None if flags & libc::O_CREAT != 0
                && unsafe { refused_change(directory, path, libc::EACCES) }.is_some() =>
            {
                fail(libc::EACCES)
            }
            None => unsafe { __real_openat(directory, path, flags, 0o666) },
        }
    }

    /// # Safety
    /// libc's contract for `stat`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_stat(path: *const c_char, buffer: *mut libc::stat) -> c_int {
        // SAFETY: libc's contract, forwarded.
        if unsafe { empty(path) } {
            return fail(libc::ENOENT);
        }
        // SAFETY: libc's contract, forwarded.
        match unsafe { entry(path) } {
            Some(entry) => stat_entry(entry, true, buffer),
            None => unsafe { __real_stat(path, buffer) },
        }
    }

    /// # Safety
    /// libc's contract for `lstat`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_lstat(path: *const c_char, buffer: *mut libc::stat) -> c_int {
        // SAFETY: libc's contract, forwarded.
        if unsafe { empty(path) } {
            return fail(libc::ENOENT);
        }
        // SAFETY: libc's contract, forwarded.
        match unsafe { entry(path) } {
            Some(entry) => stat_entry(entry, false, buffer),
            None => unsafe { __real_lstat(path, buffer) },
        }
    }

    /// # Safety
    /// libc's contract for `fstatat`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_fstatat(
        directory: c_int,
        path: *const c_char,
        buffer: *mut libc::stat,
        flags: c_int,
    ) -> c_int {
        // SAFETY: libc's contract, forwarded.
        if unsafe { empty(path) } {
            return fail(libc::ENOENT);
        }
        // SAFETY: libc's contract, forwarded.
        match unsafe { entry(path) } {
            Some(entry) => stat_entry(entry, flags & libc::AT_SYMLINK_NOFOLLOW == 0, buffer),
            None => unsafe { __real_fstatat(directory, path, buffer, flags) },
        }
    }

    /// # Safety
    /// libc's contract for `access`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_access(path: *const c_char, mode: c_int) -> c_int {
        // SAFETY: libc's contract, forwarded.
        if unsafe { empty(path) } {
            return fail(libc::ENOENT);
        }
        // SAFETY: libc's contract, forwarded.
        match unsafe { entry(path) } {
            Some(entry) => access_device(entry, mode),
            None => unsafe { __real_access(path, mode) },
        }
    }

    /// # Safety
    /// libc's contract for `faccessat`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_faccessat(
        directory: c_int,
        path: *const c_char,
        mode: c_int,
        flags: c_int,
    ) -> c_int {
        // SAFETY: libc's contract, forwarded.
        if unsafe { empty(path) } {
            return fail(libc::ENOENT);
        }
        // SAFETY: libc's contract, forwarded.
        match unsafe { entry(path) } {
            Some(entry) => access_device(entry, mode),
            None => unsafe { __real_faccessat(directory, path, mode, flags) },
        }
    }

    /// # Safety
    /// libc's contract for `readlink`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_readlink(
        path: *const c_char,
        buffer: *mut c_char,
        size: usize,
    ) -> isize {
        // SAFETY: libc's contract, forwarded.
        match unsafe { entry(path) } {
            Some(entry) => readlink_device(entry, buffer, size),
            None => unsafe { __real_readlink(path, buffer, size) },
        }
    }

    /// # Safety
    /// libc's contract for `readlinkat`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_readlinkat(
        directory: c_int,
        path: *const c_char,
        buffer: *mut c_char,
        size: usize,
    ) -> isize {
        // SAFETY: libc's contract, forwarded.
        match unsafe { entry(path) } {
            Some(entry) => readlink_device(entry, buffer, size),
            None => unsafe { __real_readlinkat(directory, path, buffer, size) },
        }
    }

    /// # Safety
    /// libc's contract for `read`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_read(fd: c_int, buffer: *mut c_void, count: usize) -> isize {
        let Some(target) = target(fd) else {
            // SAFETY: libc's contract, forwarded.
            let read = unsafe { __real_read(fd, buffer, count) };
            if read < 0 {
                directory_read_error(fd);
            }
            return read;
        };
        if count == 0 {
            return 0;
        }
        // SAFETY: libc's contract for `read`: `buffer` is writable for `count` bytes.
        let buffer = unsafe { std::slice::from_raw_parts_mut(buffer.cast::<u8>(), count) };
        answer_size(read_target(target, buffer))
    }

    /// # Safety
    /// libc's contract for `readv`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_readv(
        fd: c_int,
        iov: *const libc::iovec,
        count: c_int,
    ) -> isize {
        let Some(target) = target(fd) else {
            // SAFETY: libc's contract, forwarded.
            let read = unsafe { __real_readv(fd, iov, count) };
            if read < 0 {
                directory_read_error(fd);
            }
            return read;
        };
        // SAFETY: libc's contract for `readv`: `iov` holds `count` valid buffers.
        let buffers =
            unsafe { std::slice::from_raw_parts(iov, usize::try_from(count).unwrap_or(0)) };
        let mut total = 0;
        for buffer in buffers.iter().filter(|buffer| buffer.iov_len > 0) {
            // SAFETY: each buffer is writable for its length.
            let slice = unsafe {
                std::slice::from_raw_parts_mut(buffer.iov_base.cast::<u8>(), buffer.iov_len)
            };
            match read_target(target, slice) {
                Ok(read) => {
                    total += read;
                    if read < slice.len() {
                        break;
                    }
                }
                Err(_) if total > 0 => break,
                Err(errno) => return fail_size(errno),
            }
        }
        isize::try_from(total).unwrap_or(isize::MAX)
    }

    /// # Safety
    /// libc's contract for `write`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_write(fd: c_int, buffer: *const c_void, count: usize) -> isize {
        let Some(target) = target(fd) else {
            // SAFETY: libc's contract, forwarded.
            return unsafe { __real_write(fd, buffer, count) };
        };
        if count == 0 {
            return 0;
        }
        // SAFETY: libc's contract for `write`: `buffer` is readable for `count` bytes.
        let buffer = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), count) };
        answer_size(write_target(target, buffer))
    }

    /// # Safety
    /// libc's contract for `writev`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_writev(
        fd: c_int,
        iov: *const libc::iovec,
        count: c_int,
    ) -> isize {
        let Some(target) = target(fd) else {
            // SAFETY: libc's contract, forwarded.
            return unsafe { __real_writev(fd, iov, count) };
        };
        // SAFETY: libc's contract for `writev`: `iov` holds `count` valid buffers.
        let buffers =
            unsafe { std::slice::from_raw_parts(iov, usize::try_from(count).unwrap_or(0)) };
        let mut total = 0;
        for buffer in buffers.iter().filter(|buffer| buffer.iov_len > 0) {
            // SAFETY: each buffer is readable for its length.
            let slice =
                unsafe { std::slice::from_raw_parts(buffer.iov_base.cast::<u8>(), buffer.iov_len) };
            match write_target(target, slice) {
                Ok(written) => {
                    total += written;
                    if written < slice.len() {
                        break;
                    }
                }
                Err(_) if total > 0 => break,
                Err(errno) => return fail_size(errno),
            }
        }
        isize::try_from(total).unwrap_or(isize::MAX)
    }

    /// # Safety
    /// libc's contract for `close`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_close(fd: c_int) -> c_int {
        if ALIASES
            .with(|aliases| aliases.borrow_mut().remove(&fd))
            .is_some()
        {
            // Dropped outside the borrow: the last reference closes the shell's file, which
            // comes back here.
            let kept = REOPENED.with(|reopened| reopened.borrow_mut().remove(&fd));
            drop(kept);
            return 0;
        }
        match target(fd) {
            // A served standard descriptor closes for the command only; the process's stays.
            Some(Target::Served { level, fd }) => SERVED.with(|served| {
                match served
                    .borrow_mut()
                    .get_mut(level - 1)
                    .and_then(|served| served.files.remove(&fd))
                {
                    Some(_) => 0,
                    None => fail(libc::EBADF),
                }
            }),
            // SAFETY: libc's contract, forwarded.
            _ => unsafe { __real_close(fd) },
        }
    }

    /// # Safety
    /// libc's contract for `lseek`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_lseek(fd: c_int, offset: i64, whence: c_int) -> i64 {
        match target(fd) {
            // SAFETY: libc's contract, forwarded.
            None => unsafe { __real_lseek(fd, offset, whence) },
            Some(Target::Served { level, fd }) => match served_raw_fd(level, fd) {
                // SAFETY: libc's contract, forwarded to the file's own descriptor.
                Some(raw) => unsafe { __real_lseek(raw, offset, whence) },
                None => i64::from(fail(libc::ESPIPE)),
            },
            Some(Target::Alias(
                alias_fd,
                Alias::Reopened {
                    fd,
                    offset: at,
                    append,
                },
            )) => {
                let base = match whence {
                    libc::SEEK_SET => Ok(0),
                    libc::SEEK_CUR => Ok(at),
                    libc::SEEK_END => file_size(fd),
                    _ => Err(libc::EINVAL),
                };
                match base.map(|base| base + offset) {
                    Ok(position) if position >= 0 => {
                        set_offset(alias_fd, fd, position, append);
                        position
                    }
                    Ok(_) => i64::from(fail(libc::EINVAL)),
                    Err(errno) => i64::from(fail(errno)),
                }
            }
            Some(Target::Alias(..)) => 0,
        }
    }

    /// # Safety
    /// libc's contract for `fstat`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_fstat(fd: c_int, buffer: *mut libc::stat) -> c_int {
        let raw = match target(fd) {
            // SAFETY: libc's contract, forwarded.
            None => return unsafe { __real_fstat(fd, buffer) },
            Some(Target::Served { level, fd }) => {
                let Some(file) = served_file(level, fd) else {
                    return fail(libc::EBADF);
                };
                match file {
                    OpenFile::File(file) => Some(file.as_raw_fd()),
                    file => {
                        let null = brush_core::openfiles::is_null_sink(&file);
                        // SAFETY: an all-zero `stat` is a valid value of this plain C struct.
                        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
                        stat.st_nlink = 1;
                        if null {
                            null_stat(&mut stat);
                        } else {
                            stat.st_mode = libc::S_IFIFO | 0o600;
                        }
                        // SAFETY: libc's contract: `buffer` is writable storage for a `stat`.
                        unsafe { buffer.write(stat) };
                        return 0;
                    }
                }
            }
            Some(Target::Alias(_, Alias::Reopened { fd, .. })) => Some(fd),
            Some(Target::Alias(..)) => None,
        };
        match raw {
            // SAFETY: libc's contract, forwarded to the file's own descriptor.
            Some(raw) => unsafe { __real_fstat(raw, buffer) },
            None => {
                // SAFETY: an all-zero `stat` is a valid value of this plain C struct.
                let mut stat: libc::stat = unsafe { std::mem::zeroed() };
                stat.st_nlink = 1;
                null_stat(&mut stat);
                // SAFETY: libc's contract: `buffer` is writable storage for a `stat`.
                unsafe { buffer.write(stat) };
                0
            }
        }
    }

    /// # Safety
    /// libc's contract for `isatty`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_isatty(fd: c_int) -> c_int {
        match target(fd) {
            Some(_) => {
                fail(libc::ENOTTY);
                0
            }
            // SAFETY: libc's contract, forwarded.
            None => unsafe { __real_isatty(fd) },
        }
    }

    /// # Safety
    /// libc's contract for `ftruncate`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_ftruncate(fd: c_int, length: i64) -> c_int {
        let raw = match target(fd) {
            None => fd,
            Some(Target::Served { level, fd }) => match served_raw_fd(level, fd) {
                Some(raw) => raw,
                None => return fail(libc::EINVAL),
            },
            Some(Target::Alias(_, Alias::Reopened { fd, .. })) => fd,
            // The null device cannot be truncated.
            Some(Target::Alias(..)) => return fail(libc::EINVAL),
        };
        // SAFETY: libc's contract, forwarded.
        unsafe { __real_ftruncate(raw, length) }
    }

    /// # Safety
    /// libc's contract for `utimensat`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_utimensat(
        directory: c_int,
        path: *const c_char,
        times: *const libc::timespec,
        flags: c_int,
    ) -> c_int {
        // SAFETY: libc's contract, forwarded.
        if unsafe { empty(path) } {
            return fail(libc::ENOENT);
        }
        // SAFETY: libc's contract, forwarded.
        let entry = unsafe { absolute(directory, path) }.and_then(|path| lookup(&path));
        match entry {
            Some(entry) if entry.not_dir => fail(libc::ENOTDIR),
            // `/dev` and `/dev/fd` are not the user's to touch; `touch /dev/null` is.
            Some(entry) if entry.device == Device::Directory => fail(libc::EACCES),
            Some(_) => 0,
            // SAFETY: libc's contract, forwarded.
            None => unsafe { __real_utimensat(directory, path, times, flags) },
        }
    }

    /// # Safety
    /// libc's contract for `futimens`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_futimens(fd: c_int, times: *const libc::timespec) -> c_int {
        let raw = match target(fd) {
            None => Some(fd),
            Some(Target::Served { level, fd }) => served_raw_fd(level, fd),
            Some(Target::Alias(_, Alias::Reopened { fd, .. })) => Some(fd),
            Some(Target::Alias(..)) => None,
        };
        match raw {
            // SAFETY: libc's contract, forwarded to the file's own descriptor.
            Some(raw) => unsafe { __real_futimens(raw, times) },
            // SAFETY: libc's contract, forwarded.
            None => unsafe { device_times(times) },
        }
    }

    /// # Safety
    /// libc's contract for `unlink`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_unlink(path: *const c_char) -> c_int {
        // SAFETY: libc's contract, forwarded.
        match unsafe { refused_change(libc::AT_FDCWD, path, libc::EACCES) } {
            Some(errno) => fail(errno),
            // SAFETY: libc's contract, forwarded.
            None => unsafe { __real_unlink(path) },
        }
    }

    /// # Safety
    /// libc's contract for `unlinkat`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_unlinkat(
        directory: c_int,
        path: *const c_char,
        flags: c_int,
    ) -> c_int {
        // SAFETY: libc's contract, forwarded.
        match unsafe { refused_change(directory, path, libc::EACCES) } {
            Some(errno) => fail(errno),
            // SAFETY: libc's contract, forwarded.
            None => unsafe { __real_unlinkat(directory, path, flags) },
        }
    }

    /// # Safety
    /// libc's contract for `rmdir`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_rmdir(path: *const c_char) -> c_int {
        // SAFETY: libc's contract, forwarded.
        match unsafe { refused_change(libc::AT_FDCWD, path, libc::EACCES) } {
            Some(errno) => fail(errno),
            // SAFETY: libc's contract, forwarded.
            None => unsafe { __real_rmdir(path) },
        }
    }

    /// # Safety
    /// libc's contract for `mkdir`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_mkdir(path: *const c_char, mode: libc::mode_t) -> c_int {
        // SAFETY: libc's contract, forwarded.
        match unsafe { refused_change(libc::AT_FDCWD, path, libc::EEXIST) } {
            Some(errno) => fail(errno),
            // SAFETY: libc's contract, forwarded.
            None => unsafe { __real_mkdir(path, mode) },
        }
    }

    /// # Safety
    /// libc's contract for `linkat`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_linkat(
        old_directory: c_int,
        old: *const c_char,
        new_directory: c_int,
        new: *const c_char,
        flags: c_int,
    ) -> c_int {
        // A device is on another file system; nothing can be created inside `/dev`.
        // SAFETY: libc's contract, forwarded.
        let refused = unsafe { refused_change(old_directory, old, libc::EXDEV) }
            // SAFETY: libc's contract, forwarded.
            .or_else(|| unsafe { refused_change(new_directory, new, libc::EEXIST) });
        match refused {
            Some(errno) => fail(errno),
            // SAFETY: libc's contract, forwarded.
            None => unsafe { __real_linkat(old_directory, old, new_directory, new, flags) },
        }
    }

    /// # Safety
    /// libc's contract for `rename`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_rename(old: *const c_char, new: *const c_char) -> c_int {
        // A device cannot be moved; its copy would be a device file, which only root makes.
        // SAFETY: libc's contract, forwarded.
        let refused = unsafe { refused_change(libc::AT_FDCWD, old, libc::EPERM) }
            // SAFETY: libc's contract, forwarded.
            .or_else(|| unsafe { refused_change(libc::AT_FDCWD, new, libc::EACCES) });
        match refused {
            Some(errno) => fail(errno),
            // SAFETY: libc's contract, forwarded.
            None => unsafe { __real_rename(old, new) },
        }
    }

    /// # Safety
    /// libc's contract for `symlink`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_symlink(target: *const c_char, path: *const c_char) -> c_int {
        // SAFETY: libc's contract, forwarded.
        match unsafe { refused_change(libc::AT_FDCWD, path, libc::EEXIST) } {
            Some(errno) => fail(errno),
            // SAFETY: libc's contract, forwarded.
            None => unsafe { __real_symlink(target, path) },
        }
    }

    /// # Safety
    /// libc's contract for `opendir`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn __wrap_opendir(path: *const c_char) -> *mut libc::DIR {
        // SAFETY: libc's contract, forwarded.
        if unsafe { empty(path) } {
            fail(libc::ENOENT);
            return std::ptr::null_mut();
        }
        // The virtual directories cannot be listed.
        // SAFETY: libc's contract, forwarded.
        match unsafe { entry(path) } {
            Some(entry) => {
                fail(if entry.device == Device::Directory {
                    libc::EACCES
                } else {
                    libc::ENOTDIR
                });
                std::ptr::null_mut()
            }
            // SAFETY: libc's contract, forwarded.
            None => unsafe { __real_opendir(path) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Device, classify, lookup};
    use std::path::Path;

    #[test]
    fn a_path_past_a_device_is_not_a_directory() {
        for path in [
            "/dev/null/x",
            "/dev/stdin/x",
            "/dev/fd/1/x",
            "/proc/self/fd/0/x/y",
        ] {
            assert!(
                lookup(path.as_bytes()).is_some_and(|entry| entry.not_dir),
                "{path}"
            );
        }
        assert!(lookup(b"/dev/fd/x/y").is_none());
    }

    #[test]
    fn names_the_devices() {
        let cases = [
            ("/dev/null", Some(Device::Null)),
            ("//dev/./null", Some(Device::Null)),
            ("/tmp/../dev/null", Some(Device::Null)),
            ("/dev/stdin", Some(Device::Stream(0))),
            ("/dev/fd/1", Some(Device::Stream(1))),
            ("/proc/self/fd/2", Some(Device::Stream(2))),
            ("/dev", Some(Device::Directory)),
            ("/dev/fd/", Some(Device::Directory)),
            ("/dev/fd/63", Some(Device::Descriptor(63))),
            ("/dev/fd/x", None),
            ("dev/null", None),
            ("/devnull", None),
            ("/tmp/dev/null", None),
            ("/dev/null/x", None),
            ("/dev/stdin/x/y", None),
            ("/dev/fd/1/x", None),
        ];
        for (path, expected) in cases {
            assert_eq!(classify(Path::new(path)), expected, "{path}");
        }
    }

    #[test]
    fn stream_names_are_links() {
        assert_eq!(
            lookup(b"/dev/stdin").and_then(|entry| entry.link),
            Some("/proc/self/fd/0")
        );
        assert_eq!(lookup(b"/dev/fd/0").and_then(|entry| entry.link), None);
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod served_tests {
    use super::serve;
    use brush_core::openfiles::{OpenFile, from_bytes};
    use std::io::{Read, Seek, Write};

    fn served(files: Vec<(i32, OpenFile)>) -> super::wasm::Serving {
        serve(files.into_iter().collect(), false)
    }

    #[test]
    fn dev_stdin_shares_the_served_input_position() {
        let _served = served(vec![(0, from_bytes(b"ab".to_vec()))]);
        let mut first = std::fs::File::open("/dev/stdin").unwrap();
        let mut second = std::fs::File::open("/dev/fd/0").unwrap();
        let mut byte = [0; 1];
        first.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"a");
        second.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"b");
        assert_eq!(first.read(&mut byte).unwrap(), 0);
    }

    #[test]
    fn standard_descriptors_are_the_served_streams() {
        let (sink, output) = brush_core::openfiles::memory_sink(1024);
        let _served = served(vec![(0, from_bytes(b"in".to_vec())), (1, sink)]);
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).unwrap();
        assert_eq!(input, "in");
        let mut stdout = std::io::stdout();
        stdout.write_all(b"out").unwrap();
        stdout.flush().unwrap();
        assert_eq!(output.lock().unwrap().bytes, b"out");
    }

    #[test]
    fn a_regular_file_descriptor_opens_again_with_its_own_position() {
        let path = crate::tools::test_scratch("devfd");
        std::fs::write(&path, b"file\n").unwrap();
        let shell_file = std::sync::Arc::new(std::fs::File::open(&path).unwrap());
        let _served = served(vec![(5, OpenFile::File(shell_file.clone()))]);
        let mut reopened = String::new();
        std::fs::File::open("/dev/fd/5")
            .unwrap()
            .read_to_string(&mut reopened)
            .unwrap();
        assert_eq!(reopened, "file\n");
        // The shell's own descriptor has not moved.
        assert_eq!((&*shell_file).stream_position().unwrap(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_reopened_file_reads_from_its_start_and_leaves_the_shell_position() {
        let path = crate::tools::test_scratch("reopen");
        std::fs::write(&path, b"b\na\n").unwrap();
        let shell_file = std::sync::Arc::new(std::fs::File::open(&path).unwrap());
        let mut line = [0; 2];
        (&*shell_file).read_exact(&mut line).unwrap();
        let mut reopened = super::reopen(shell_file.clone());
        let mut text = String::new();
        reopened.read_to_string(&mut text).unwrap();
        assert_eq!(text, "b\na\n");
        assert_eq!(reopened.stream_position().unwrap(), 4);
        drop(reopened);
        assert_eq!((&*shell_file).stream_position().unwrap(), 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn closed_and_unnamed_descriptors_do_not_exist() {
        let _served = served(vec![(1, from_bytes(Vec::new()))]);
        assert!(std::fs::metadata("/dev/fd/7").is_err());
        assert!(std::fs::File::open("/dev/fd/7").is_err());
        assert!(std::fs::metadata("/dev/stdout").is_ok());
    }

    #[test]
    fn dev_null_keeps_nothing_without_a_filesystem() {
        let mut null = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")
            .unwrap();
        null.write_all(b"gone").unwrap();
        let mut bytes = Vec::new();
        assert_eq!(null.read_to_end(&mut bytes).unwrap(), 0);
    }
}
