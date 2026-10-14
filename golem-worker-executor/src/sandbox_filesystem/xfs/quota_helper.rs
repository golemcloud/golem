// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! The XFS quota helper: the only code that calls `quotactl_fd`.
//!
//! Linux allows the project quota calls of `ManagedXfs` storage only with `CAP_SYS_ADMIN`. The
//! executor runs guest Wasm, so it does not hold that capability. It sends each quota call to a
//! helper process over a Unix socket instead. The helper serves one XFS root, holds no capability
//! other than `CAP_SYS_ADMIN`, runs no guest code, and accepts three calls: read the quota state of
//! the volume, read the quota of a project, and set the limits of a project.
//!
//! The helper runs in one of two ways:
//!
//! - The executor starts it as a child process, from its own binary, before it starts any thread
//!   ([`start_for_storage`]). The child gets `CAP_SYS_ADMIN` the way the executor got it: from the
//!   file capability of the binary, or as root. The executor then drops the capability.
//! - An operator runs it beside the executor, for example as a sidecar container
//!   (`xfs-quota-helper --root <root> --listen <socket>`), and configures the socket in
//!   `ManagedXfs` mode. The executor then needs no capability at all.
//!
//! On connect, the helper sends a greeting with the protocol version and the device of its root,
//! and the client refuses a helper of another version or of another filesystem.

use super::{
    FS_DQUOT_VERSION, FS_PROJ_QUOTA, FsDiskQuota, FsQuotaFileStatV, FsQuotaStatV,
    PROJECT_LIMIT_FIELDS, Q_XGETQSTATV, Q_XGETQUOTA, Q_XSETQLIM, XFS_SUPER_MAGIC, XQM_PRJQUOTA,
};
use anyhow::{Context, anyhow, bail};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex, OnceLock};

/// The first argument that runs the binary as the quota helper.
pub const HELPER_COMMAND: &str = "xfs-quota-helper";

/// The descriptor on which a helper that the executor starts finds its connection.
const CHILD_CONNECTION_FD: RawFd = 3;

const GREETING_MAGIC: [u8; 4] = *b"GQH1";
const GREETING_READY: u8 = 0;
const GREETING_REFUSED: u8 = 1;

const OPERATION_QUOTA_STATE: u8 = 1;
const OPERATION_PROJECT_QUOTA: u8 = 2;
const OPERATION_SET_PROJECT_LIMITS: u8 = 3;

const QUOTA_BYTES: usize = 112;
const QUOTA_STATE_BYTES: usize = 160;
const REQUEST_BYTES: usize = 1 + 4 + QUOTA_BYTES;
const RESPONSE_BYTES: usize = 4 + QUOTA_STATE_BYTES;

/// A connection to the quota helper of one XFS root.
pub struct XfsQuotaHelper {
    connection: Mutex<Connection>,
}

struct Connection {
    endpoint: Endpoint,
    /// The device of the XFS root that the helper must serve.
    device: u64,
    stream: Option<UnixStream>,
}

enum Endpoint {
    /// A connection that the executor made when it started the helper. It cannot be made again.
    Started,
    /// A socket of a helper that runs beside the executor.
    Socket(PathBuf),
}

impl XfsQuotaHelper {
    /// Connects to the helper that listens on `socket`, and checks that it serves the filesystem
    /// with the device `device`.
    pub(crate) fn connect(socket: &Path, device: u64) -> std::io::Result<Self> {
        let stream = connect_socket(socket, device)?;
        Ok(Self {
            connection: Mutex::new(Connection {
                endpoint: Endpoint::Socket(socket.to_path_buf()),
                device,
                stream: Some(stream),
            }),
        })
    }

    /// Wraps the connected `stream` of a started helper whose greeting was read already.
    fn started(stream: UnixStream, device: u64) -> Self {
        Self {
            connection: Mutex::new(Connection {
                endpoint: Endpoint::Started,
                device,
                stream: Some(stream),
            }),
        }
    }

    fn device(&self) -> u64 {
        self.connection
            .lock()
            .expect("XFS quota helper lock poisoned")
            .device
    }

    pub(super) fn get_quota_state(&self, state: &mut FsQuotaStatV) -> std::io::Result<()> {
        let mut request = [0u8; REQUEST_BYTES];
        request[0] = OPERATION_QUOTA_STATE;
        let response = self.call(&request)?;
        *state = decode_quota_state(&response[..QUOTA_STATE_BYTES]);
        Ok(())
    }

    pub(super) fn get_project_quota(
        &self,
        project_id: u32,
        quota: &mut FsDiskQuota,
    ) -> std::io::Result<()> {
        let request = encode_request(OPERATION_PROJECT_QUOTA, project_id, &FsDiskQuota::default());
        let response = self.call(&request)?;
        *quota = decode_quota(&response[..QUOTA_BYTES]);
        Ok(())
    }

    pub(super) fn set_project_quota_record(
        &self,
        project_id: u32,
        quota: &FsDiskQuota,
    ) -> std::io::Result<()> {
        let request = encode_request(OPERATION_SET_PROJECT_LIMITS, project_id, quota);
        self.call(&request).map(|_| ())
    }

    /// Sends `request` and gives the payload of the response, or the error of the call. A helper
    /// that listens on a socket is connected again once when the connection breaks; every call is
    /// idempotent, so a repeat cannot apply a change twice.
    fn call(&self, request: &[u8; REQUEST_BYTES]) -> std::io::Result<[u8; QUOTA_STATE_BYTES]> {
        let mut connection = self
            .connection
            .lock()
            .expect("XFS quota helper lock poisoned");
        match connection.exchange(request) {
            Err(Transport(error)) => {
                connection.stream = None;
                match &connection.endpoint {
                    Endpoint::Started => Err(helper_lost(error)),
                    Endpoint::Socket(socket) => {
                        let socket = socket.clone();
                        connection.stream = Some(connect_socket(&socket, connection.device)?);
                        match connection.exchange(request) {
                            Err(Transport(error)) => {
                                connection.stream = None;
                                Err(helper_lost(error))
                            }
                            Ok(result) => result,
                        }
                    }
                }
            }
            Ok(result) => result,
        }
    }
}

/// A failure to reach the helper, as opposed to an error of the call that the helper made.
struct Transport(std::io::Error);

impl Connection {
    fn exchange(
        &mut self,
        request: &[u8; REQUEST_BYTES],
    ) -> Result<std::io::Result<[u8; QUOTA_STATE_BYTES]>, Transport> {
        let stream = match &mut self.stream {
            Some(stream) => stream,
            None => {
                return Err(Transport(std::io::Error::new(
                    ErrorKind::NotConnected,
                    "the XFS quota helper connection is closed",
                )));
            }
        };
        stream.write_all(request).map_err(Transport)?;
        let mut response = [0u8; RESPONSE_BYTES];
        stream.read_exact(&mut response).map_err(Transport)?;
        let errno = i32::from_le_bytes(response[..4].try_into().unwrap());
        let mut payload = [0u8; QUOTA_STATE_BYTES];
        payload.copy_from_slice(&response[4..]);
        Ok(if errno == 0 {
            Ok(payload)
        } else {
            Err(std::io::Error::from_raw_os_error(errno))
        })
    }
}

fn helper_lost(error: std::io::Error) -> std::io::Error {
    std::io::Error::new(
        error.kind(),
        format!("lost the connection to the XFS quota helper: {error}"),
    )
}

fn connect_socket(socket: &Path, device: u64) -> std::io::Result<UnixStream> {
    let mut stream = UnixStream::connect(socket).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!(
                "failed to connect to the XFS quota helper at {}: {error}",
                socket.display()
            ),
        )
    })?;
    read_greeting(&mut stream, device)?;
    Ok(stream)
}

/// Reads the greeting of the helper on `stream`, and checks that the helper is ready and serves
/// the filesystem with the device `device`.
fn read_greeting(stream: &mut impl Read, device: u64) -> std::io::Result<()> {
    let mut header = [0u8; 4 + 1 + 8 + 2];
    stream.read_exact(&mut header).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("the XFS quota helper sent no greeting: {error}"),
        )
    })?;
    if header[..4] != GREETING_MAGIC {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            "the XFS quota helper speaks another protocol version; run the helper and the \
             executor from the same Golem release",
        ));
    }
    let status = header[4];
    let helper_device = u64::from_le_bytes(header[5..13].try_into().unwrap());
    let message_length = u16::from_le_bytes(header[13..15].try_into().unwrap()) as usize;
    let mut message = vec![0u8; message_length];
    stream.read_exact(&mut message)?;
    let message = String::from_utf8_lossy(&message);
    if status != GREETING_READY {
        return Err(std::io::Error::new(
            ErrorKind::PermissionDenied,
            format!("the XFS quota helper refused to start: {message}"),
        ));
    }
    if helper_device != device {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "the XFS quota helper serves another filesystem than the executor's XFS root",
        ));
    }
    Ok(())
}

fn write_greeting(
    stream: &mut impl Write,
    refusal: Option<&str>,
    device: u64,
) -> std::io::Result<()> {
    let message = refusal.unwrap_or("").as_bytes();
    let message = &message[..message.len().min(u16::MAX as usize)];
    let mut greeting = Vec::with_capacity(15 + message.len());
    greeting.extend_from_slice(&GREETING_MAGIC);
    greeting.push(if refusal.is_some() {
        GREETING_REFUSED
    } else {
        GREETING_READY
    });
    greeting.extend_from_slice(&device.to_le_bytes());
    greeting.extend_from_slice(&(message.len() as u16).to_le_bytes());
    greeting.extend_from_slice(message);
    stream.write_all(&greeting)
}

// --- The helpers of each XFS root that this process uses ---

fn helpers() -> &'static Mutex<HashMap<PathBuf, Arc<XfsQuotaHelper>>> {
    static HELPERS: OnceLock<Mutex<HashMap<PathBuf, Arc<XfsQuotaHelper>>>> = OnceLock::new();
    HELPERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register(root: &Path, helper: XfsQuotaHelper) {
    helpers()
        .lock()
        .expect("XFS quota helper registry lock poisoned")
        .insert(root.to_path_buf(), Arc::new(helper));
}

/// The helper for the XFS root `root`, whose filesystem has the device `device`: the helper on
/// `socket` when one is configured, otherwise the helper that this process started for `root`.
pub(super) fn helper_for_root(
    root: &Path,
    socket: Option<&Path>,
    device: u64,
) -> std::io::Result<Arc<XfsQuotaHelper>> {
    if let Some(socket) = socket {
        return XfsQuotaHelper::connect(socket, device).map(Arc::new);
    }
    let registered = helpers()
        .lock()
        .expect("XFS quota helper registry lock poisoned")
        .get(root)
        .cloned();
    #[cfg(test)]
    let registered = match registered {
        Some(helper) => Some(helper),
        None => {
            start_in_process(root)?;
            return helper_for_root(root, None, device);
        }
    };
    match registered {
        Some(helper) if helper.device() == device => Ok(helper),
        Some(_) => Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "the XFS quota helper serves another filesystem than the executor's XFS root",
        )),
        None => Err(std::io::Error::new(
            ErrorKind::NotFound,
            "no XFS quota helper serves this root: configure the socket of a helper, or start \
             the worker-executor binary, which starts a helper for ManagedXfs storage",
        )),
    }
}

// --- Starting a helper ---

/// A helper that this process started as a child process.
pub struct StartedHelper {
    child: Child,
}

impl StartedHelper {
    /// Waits until the helper exits, and gives a description of how it ended.
    pub fn wait(mut self) -> String {
        match self.child.wait() {
            Ok(status) => format!("the XFS quota helper exited with {status}"),
            Err(error) => format!("failed to wait for the XFS quota helper: {error}"),
        }
    }
}

/// Starts the quota helper for `ManagedXfs` storage without a configured helper socket, and gives
/// it to the storage of this process. Does nothing for another storage mode.
///
/// The helper is this binary, run again with [`HELPER_COMMAND`]. It gets `CAP_SYS_ADMIN` from the
/// file capability of the binary, or as root. It refuses to start without it, and this function
/// then fails with the reason.
pub fn start_for_storage(
    mode: &crate::services::golem_config::FilesystemStorageMode,
) -> anyhow::Result<Option<StartedHelper>> {
    let crate::services::golem_config::FilesystemStorageMode::ManagedXfs {
        root,
        quota_helper_socket: None,
    } = mode
    else {
        return Ok(None);
    };
    let device = root_device(root)?;
    let (parent, child_end) =
        UnixStream::pair().context("failed to create the XFS quota helper connection")?;
    let child_fd = child_end.as_raw_fd();
    let mut command = Command::new("/proc/self/exe");
    command
        .arg(HELPER_COMMAND)
        .arg("--root")
        .arg(&**root)
        .arg("--fd")
        .arg(CHILD_CONNECTION_FD.to_string());
    // SAFETY: `dup2` and `fcntl` are async-signal-safe. They give the child its end of the
    // connection on a fixed descriptor that stays open across `exec`. `dup2` onto the same
    // descriptor keeps its close-on-exec flag, so that case clears the flag itself.
    unsafe {
        command.pre_exec(move || {
            let result = if child_fd == CHILD_CONNECTION_FD {
                libc::fcntl(child_fd, libc::F_SETFD, 0)
            } else {
                libc::dup2(child_fd, CHILD_CONNECTION_FD)
            };
            if result == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command
        .spawn()
        .context("failed to start the XFS quota helper")?;
    drop(child_end);
    let mut parent = parent;
    read_greeting(&mut parent, device).map_err(|error| anyhow!("{error}"))?;
    register(root, XfsQuotaHelper::started(parent, device));
    Ok(Some(StartedHelper { child }))
}

/// Serves the quota calls for the XFS root `root` on a thread of this process, and gives the
/// helper to the storage of this process. The process must hold `CAP_SYS_ADMIN` itself; this is
/// for a process that runs no guest code, such as a test.
pub fn start_in_process(root: &Path) -> std::io::Result<()> {
    let root_file = open_root(root).map_err(std::io::Error::other)?;
    let device = root_file.metadata()?.dev();
    let (mut client, server) = UnixStream::pair()?;
    std::thread::Builder::new()
        .name("xfs-quota-helper".to_string())
        .spawn(move || {
            let mut server = server;
            if write_greeting(&mut server, None, device).is_ok() {
                serve(&root_file, server);
            }
        })?;
    read_greeting(&mut client, device)?;
    register(root, XfsQuotaHelper::started(client, device));
    Ok(())
}

fn root_device(root: &Path) -> anyhow::Result<u64> {
    Ok(std::fs::metadata(root)
        .with_context(|| format!("failed to inspect the XFS root {}", root.display()))?
        .dev())
}

// --- The helper process ---

/// Runs the quota helper when the first argument of the process is [`HELPER_COMMAND`], and gives
/// its result. Gives `None` for any other command line.
pub fn run_if_requested() -> Option<anyhow::Result<()>> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != HELPER_COMMAND {
        return None;
    }
    Some(
        HelperArgs::parse(args)
            .and_then(run_helper)
            .context("XFS quota helper"),
    )
}

struct HelperArgs {
    root: PathBuf,
    connection: HelperConnection,
}

enum HelperConnection {
    Descriptor(RawFd),
    Listen(PathBuf),
}

impl HelperArgs {
    fn parse(mut args: impl Iterator<Item = OsString>) -> anyhow::Result<Self> {
        let usage = || {
            anyhow!(
                "usage: {HELPER_COMMAND} --root <XFS root> (--listen <socket> | --fd <descriptor>)"
            )
        };
        let mut root = None;
        let mut connection = None;
        while let Some(arg) = args.next() {
            let value = args.next().ok_or_else(usage)?;
            match arg.to_str() {
                Some("--root") => root = Some(PathBuf::from(value)),
                Some("--listen") => connection = Some(HelperConnection::Listen(value.into())),
                Some("--fd") => {
                    let fd = value
                        .to_str()
                        .and_then(|value| value.parse().ok())
                        .ok_or_else(usage)?;
                    connection = Some(HelperConnection::Descriptor(fd))
                }
                _ => return Err(usage()),
            }
        }
        Ok(Self {
            root: root.ok_or_else(usage)?,
            connection: connection.ok_or_else(usage)?,
        })
    }
}

fn run_helper(args: HelperArgs) -> anyhow::Result<()> {
    let setup = crate::process_capabilities::retain_only_sys_admin().and_then(|()| {
        let root = open_root(&args.root)?;
        let device = root.metadata()?.dev();
        Ok((root, device))
    });
    match args.connection {
        HelperConnection::Descriptor(fd) => {
            // SAFETY: The executor gives the helper its end of the connection on this descriptor,
            // and nothing else in this process uses it.
            let mut stream = unsafe { UnixStream::from_raw_fd(fd) };
            match setup {
                Ok((root, device)) => {
                    write_greeting(&mut stream, None, device)?;
                    serve(&root, stream);
                    Ok(())
                }
                Err(error) => {
                    let _ = write_greeting(&mut stream, Some(&format!("{error:#}")), 0);
                    Err(error)
                }
            }
        }
        HelperConnection::Listen(socket) => {
            let (root, device) = setup?;
            listen(Arc::new(root), device, &socket)
        }
    }
}

/// Opens the XFS root `root`. The helper does not lock the root: the executor holds that lock.
fn open_root(root: &Path) -> anyhow::Result<File> {
    let file = File::open(root)
        .with_context(|| format!("failed to open the XFS root {}", root.display()))?;
    let filesystem = rustix::fs::fstatfs(&file)
        .with_context(|| format!("failed to inspect the XFS root {}", root.display()))?;
    if filesystem.f_type as u64 != XFS_SUPER_MAGIC {
        bail!("{} is not on an XFS filesystem", root.display());
    }
    Ok(file)
}

/// Accepts connections on `socket` from processes of the user of this process only, and serves
/// each on its own thread.
fn listen(root: Arc<File>, device: u64, socket: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(socket) {
        Err(error) if error.kind() != ErrorKind::NotFound => {
            return Err(error).with_context(|| format!("failed to remove {}", socket.display()));
        }
        _ => {}
    }
    let listener = UnixListener::bind(socket)
        .with_context(|| format!("failed to listen on {}", socket.display()))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to restrict {}", socket.display()))?;
    let uid = rustix::process::geteuid();
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        match rustix::net::sockopt::socket_peercred(&stream) {
            Ok(peer) if peer.uid == uid => {}
            _ => continue,
        }
        let root = Arc::clone(&root);
        std::thread::spawn(move || {
            if write_greeting(&mut stream, None, device).is_ok() {
                serve(&root, stream);
            }
        });
    }
    Ok(())
}

/// Answers the calls on `stream` until the client closes it.
fn serve(root: &File, mut stream: UnixStream) {
    let mut request = [0u8; REQUEST_BYTES];
    while stream.read_exact(&mut request).is_ok() {
        let response = answer(root, &request);
        if stream.write_all(&response).is_err() {
            return;
        }
    }
}

fn answer(root: &File, request: &[u8; REQUEST_BYTES]) -> [u8; RESPONSE_BYTES] {
    let mut response = [0u8; RESPONSE_BYTES];
    let project_id = u32::from_le_bytes(request[1..5].try_into().unwrap());
    let result = match request[0] {
        OPERATION_QUOTA_STATE => {
            let mut state = FsQuotaStatV {
                qs_version: super::FS_QSTATV_VERSION1,
                ..FsQuotaStatV::default()
            };
            // SAFETY: Q_XGETQSTATV writes exactly one `fs_quota_statv` record.
            unsafe { quotactl(root, Q_XGETQSTATV, 0, std::ptr::from_mut(&mut state).cast()) }
                .map(|()| encode_quota_state(&state, &mut response[4..]))
        }
        OPERATION_PROJECT_QUOTA => {
            let mut quota = FsDiskQuota::default();
            // SAFETY: Q_XGETQUOTA writes exactly one `fs_disk_quota` record.
            unsafe {
                quotactl(
                    root,
                    Q_XGETQUOTA,
                    project_id,
                    std::ptr::from_mut(&mut quota).cast(),
                )
            }
            .map(|()| encode_quota(&quota, &mut response[4..4 + QUOTA_BYTES]))
        }
        OPERATION_SET_PROJECT_LIMITS => {
            let mut quota = decode_quota(&request[5..]);
            if is_project_limit_change(&quota, project_id) {
                // SAFETY: Q_XSETQLIM reads exactly one `fs_disk_quota` record.
                unsafe {
                    quotactl(
                        root,
                        Q_XSETQLIM,
                        project_id,
                        std::ptr::from_mut(&mut quota).cast(),
                    )
                }
            } else {
                Err(std::io::Error::from_raw_os_error(libc::EINVAL))
            }
        }
        _ => Err(std::io::Error::from_raw_os_error(libc::EINVAL)),
    };
    let errno = match result {
        Ok(()) => 0,
        Err(error) => error.raw_os_error().unwrap_or(libc::EIO),
    };
    response[..4].copy_from_slice(&errno.to_le_bytes());
    response
}

/// Whether `quota` only changes limits of the project `project_id`, which is not project 0.
fn is_project_limit_change(quota: &FsDiskQuota, project_id: u32) -> bool {
    project_id != 0
        && quota.d_version == FS_DQUOT_VERSION
        && quota.d_flags == FS_PROJ_QUOTA
        && quota.d_id == project_id
        && quota.d_fieldmask & !PROJECT_LIMIT_FIELDS == 0
}

/// # Safety
///
/// `data` must point to the UAPI structure that `command` reads or writes, for the duration of
/// the call.
unsafe fn quotactl(
    root: &File,
    command: u32,
    id: u32,
    data: *mut libc::c_void,
) -> std::io::Result<()> {
    let operation = (command << 8) | (XQM_PRJQUOTA & 0xff);
    // SAFETY: The caller guarantees that `data` points to the structure that `command` uses.
    let result =
        unsafe { libc::syscall(libc::SYS_quotactl_fd, root.as_raw_fd(), operation, id, data) };
    if result == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

// --- Encoding ---

fn encode_request(operation: u8, project_id: u32, quota: &FsDiskQuota) -> [u8; REQUEST_BYTES] {
    let mut request = [0u8; REQUEST_BYTES];
    request[0] = operation;
    request[1..5].copy_from_slice(&project_id.to_le_bytes());
    encode_quota(quota, &mut request[5..]);
    request
}

struct Writer<'a> {
    bytes: &'a mut [u8],
    at: usize,
}

impl Writer<'_> {
    fn put<const N: usize>(&mut self, value: [u8; N]) {
        self.bytes[self.at..self.at + N].copy_from_slice(&value);
        self.at += N;
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let value = self.bytes[self.at..self.at + N].try_into().unwrap();
        self.at += N;
        value
    }
}

fn encode_quota(quota: &FsDiskQuota, bytes: &mut [u8]) {
    let mut w = Writer { bytes, at: 0 };
    w.put(quota.d_version.to_le_bytes());
    w.put(quota.d_flags.to_le_bytes());
    w.put(quota.d_fieldmask.to_le_bytes());
    w.put(quota.d_id.to_le_bytes());
    w.put(quota.d_blk_hardlimit.to_le_bytes());
    w.put(quota.d_blk_softlimit.to_le_bytes());
    w.put(quota.d_ino_hardlimit.to_le_bytes());
    w.put(quota.d_ino_softlimit.to_le_bytes());
    w.put(quota.d_bcount.to_le_bytes());
    w.put(quota.d_icount.to_le_bytes());
    w.put(quota.d_itimer.to_le_bytes());
    w.put(quota.d_btimer.to_le_bytes());
    w.put(quota.d_iwarns.to_le_bytes());
    w.put(quota.d_bwarns.to_le_bytes());
    w.put(quota.d_itimer_hi.to_le_bytes());
    w.put(quota.d_btimer_hi.to_le_bytes());
    w.put(quota.d_rtbtimer_hi.to_le_bytes());
    w.put(quota.d_padding2.to_le_bytes());
    w.put(quota.d_rtb_hardlimit.to_le_bytes());
    w.put(quota.d_rtb_softlimit.to_le_bytes());
    w.put(quota.d_rtbcount.to_le_bytes());
    w.put(quota.d_rtbtimer.to_le_bytes());
    w.put(quota.d_rtbwarns.to_le_bytes());
    w.put(quota.d_padding3.to_le_bytes());
    w.put(quota.d_padding4.map(|byte| byte as u8));
    debug_assert_eq!(w.at, QUOTA_BYTES);
}

fn decode_quota(bytes: &[u8]) -> FsDiskQuota {
    let mut r = Reader { bytes, at: 0 };
    let quota = FsDiskQuota {
        d_version: i8::from_le_bytes(r.take()),
        d_flags: i8::from_le_bytes(r.take()),
        d_fieldmask: u16::from_le_bytes(r.take()),
        d_id: u32::from_le_bytes(r.take()),
        d_blk_hardlimit: u64::from_le_bytes(r.take()),
        d_blk_softlimit: u64::from_le_bytes(r.take()),
        d_ino_hardlimit: u64::from_le_bytes(r.take()),
        d_ino_softlimit: u64::from_le_bytes(r.take()),
        d_bcount: u64::from_le_bytes(r.take()),
        d_icount: u64::from_le_bytes(r.take()),
        d_itimer: i32::from_le_bytes(r.take()),
        d_btimer: i32::from_le_bytes(r.take()),
        d_iwarns: u16::from_le_bytes(r.take()),
        d_bwarns: u16::from_le_bytes(r.take()),
        d_itimer_hi: i8::from_le_bytes(r.take()),
        d_btimer_hi: i8::from_le_bytes(r.take()),
        d_rtbtimer_hi: i8::from_le_bytes(r.take()),
        d_padding2: i8::from_le_bytes(r.take()),
        d_rtb_hardlimit: u64::from_le_bytes(r.take()),
        d_rtb_softlimit: u64::from_le_bytes(r.take()),
        d_rtbcount: u64::from_le_bytes(r.take()),
        d_rtbtimer: i32::from_le_bytes(r.take()),
        d_rtbwarns: u16::from_le_bytes(r.take()),
        d_padding3: i16::from_le_bytes(r.take()),
        d_padding4: r.take::<8>().map(|byte| byte as i8),
    };
    debug_assert_eq!(r.at, QUOTA_BYTES);
    quota
}

fn encode_quota_file(file: &FsQuotaFileStatV, w: &mut Writer<'_>) {
    w.put(file.qfs_ino.to_le_bytes());
    w.put(file.qfs_nblks.to_le_bytes());
    w.put(file.qfs_nextents.to_le_bytes());
    w.put(file.qfs_pad.to_le_bytes());
}

fn decode_quota_file(r: &mut Reader<'_>) -> FsQuotaFileStatV {
    FsQuotaFileStatV {
        qfs_ino: u64::from_le_bytes(r.take()),
        qfs_nblks: u64::from_le_bytes(r.take()),
        qfs_nextents: u32::from_le_bytes(r.take()),
        qfs_pad: u32::from_le_bytes(r.take()),
    }
}

fn encode_quota_state(state: &FsQuotaStatV, bytes: &mut [u8]) {
    let mut w = Writer { bytes, at: 0 };
    w.put(state.qs_version.to_le_bytes());
    w.put(state.qs_pad1.to_le_bytes());
    w.put(state.qs_flags.to_le_bytes());
    w.put(state.qs_incoredqs.to_le_bytes());
    encode_quota_file(&state.qs_uquota, &mut w);
    encode_quota_file(&state.qs_gquota, &mut w);
    encode_quota_file(&state.qs_pquota, &mut w);
    w.put(state.qs_btimelimit.to_le_bytes());
    w.put(state.qs_itimelimit.to_le_bytes());
    w.put(state.qs_rtbtimelimit.to_le_bytes());
    w.put(state.qs_bwarnlimit.to_le_bytes());
    w.put(state.qs_iwarnlimit.to_le_bytes());
    w.put(state.qs_rtbwarnlimit.to_le_bytes());
    w.put(state.qs_pad3.to_le_bytes());
    w.put(state.qs_pad4.to_le_bytes());
    for pad in state.qs_pad2 {
        w.put(pad.to_le_bytes());
    }
    debug_assert_eq!(w.at, QUOTA_STATE_BYTES);
}

fn decode_quota_state(bytes: &[u8]) -> FsQuotaStatV {
    let mut r = Reader { bytes, at: 0 };
    let state = FsQuotaStatV {
        qs_version: i8::from_le_bytes(r.take()),
        qs_pad1: u8::from_le_bytes(r.take()),
        qs_flags: u16::from_le_bytes(r.take()),
        qs_incoredqs: u32::from_le_bytes(r.take()),
        qs_uquota: decode_quota_file(&mut r),
        qs_gquota: decode_quota_file(&mut r),
        qs_pquota: decode_quota_file(&mut r),
        qs_btimelimit: i32::from_le_bytes(r.take()),
        qs_itimelimit: i32::from_le_bytes(r.take()),
        qs_rtbtimelimit: i32::from_le_bytes(r.take()),
        qs_bwarnlimit: u16::from_le_bytes(r.take()),
        qs_iwarnlimit: u16::from_le_bytes(r.take()),
        qs_rtbwarnlimit: u16::from_le_bytes(r.take()),
        qs_pad3: u16::from_le_bytes(r.take()),
        qs_pad4: u32::from_le_bytes(r.take()),
        qs_pad2: std::array::from_fn(|_| u64::from_le_bytes(r.take())),
    };
    debug_assert_eq!(r.at, QUOTA_STATE_BYTES);
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::{assert, let_assert};
    use test_r::test;

    fn sample_quota() -> FsDiskQuota {
        FsDiskQuota {
            d_version: FS_DQUOT_VERSION,
            d_flags: FS_PROJ_QUOTA,
            d_fieldmask: PROJECT_LIMIT_FIELDS,
            d_id: 0x8000_0001,
            d_blk_hardlimit: 1,
            d_blk_softlimit: 2,
            d_ino_hardlimit: 3,
            d_ino_softlimit: 4,
            d_bcount: 5,
            d_icount: 6,
            d_itimer: -7,
            d_btimer: 8,
            d_iwarns: 9,
            d_bwarns: 10,
            d_itimer_hi: -11,
            d_btimer_hi: 12,
            d_rtbtimer_hi: 13,
            d_padding2: 14,
            d_rtb_hardlimit: 15,
            d_rtb_softlimit: 16,
            d_rtbcount: u64::MAX,
            d_rtbtimer: 18,
            d_rtbwarns: 19,
            d_padding3: -20,
            d_padding4: [1, -2, 3, -4, 5, -6, 7, -8],
        }
    }

    fn quota_fields(quota: &FsDiskQuota) -> Vec<i128> {
        vec![
            quota.d_version as i128,
            quota.d_flags as i128,
            quota.d_fieldmask as i128,
            quota.d_id as i128,
            quota.d_blk_hardlimit as i128,
            quota.d_blk_softlimit as i128,
            quota.d_ino_hardlimit as i128,
            quota.d_ino_softlimit as i128,
            quota.d_bcount as i128,
            quota.d_icount as i128,
            quota.d_itimer as i128,
            quota.d_btimer as i128,
            quota.d_iwarns as i128,
            quota.d_bwarns as i128,
            quota.d_itimer_hi as i128,
            quota.d_btimer_hi as i128,
            quota.d_rtbtimer_hi as i128,
            quota.d_padding2 as i128,
            quota.d_rtb_hardlimit as i128,
            quota.d_rtb_softlimit as i128,
            quota.d_rtbcount as i128,
            quota.d_rtbtimer as i128,
            quota.d_rtbwarns as i128,
            quota.d_padding3 as i128,
        ]
        .into_iter()
        .chain(quota.d_padding4.iter().map(|byte| *byte as i128))
        .collect()
    }

    #[test]
    fn a_quota_record_survives_encoding() {
        let quota = sample_quota();
        let mut bytes = [0u8; QUOTA_BYTES];
        encode_quota(&quota, &mut bytes);
        assert!(quota_fields(&decode_quota(&bytes)) == quota_fields(&quota));
    }

    #[test]
    fn a_quota_state_survives_encoding() {
        let file = |base: u64| FsQuotaFileStatV {
            qfs_ino: base,
            qfs_nblks: base + 1,
            qfs_nextents: base as u32 + 2,
            qfs_pad: base as u32 + 3,
        };
        let state = FsQuotaStatV {
            qs_version: super::super::FS_QSTATV_VERSION1,
            qs_pad1: 1,
            qs_flags: 0x30,
            qs_incoredqs: 2,
            qs_uquota: file(10),
            qs_gquota: file(20),
            qs_pquota: file(30),
            qs_btimelimit: -1,
            qs_itimelimit: 3,
            qs_rtbtimelimit: 4,
            qs_bwarnlimit: 5,
            qs_iwarnlimit: 6,
            qs_rtbwarnlimit: 7,
            qs_pad3: 8,
            qs_pad4: 9,
            qs_pad2: [1, 2, 3, 4, 5, 6, u64::MAX],
        };
        let mut bytes = [0u8; QUOTA_STATE_BYTES];
        encode_quota_state(&state, &mut bytes);
        let mut again = [0u8; QUOTA_STATE_BYTES];
        encode_quota_state(&decode_quota_state(&bytes), &mut again);
        assert!(bytes == again);
        let decoded = decode_quota_state(&bytes);
        assert!(decoded.qs_flags == 0x30);
        assert!(decoded.qs_pquota.qfs_nblks == 31);
        assert!(decoded.qs_pad2[6] == u64::MAX);
    }

    #[test]
    fn only_limit_changes_of_a_nonzero_project_are_accepted() {
        let change = FsDiskQuota {
            d_version: FS_DQUOT_VERSION,
            d_flags: FS_PROJ_QUOTA,
            d_fieldmask: PROJECT_LIMIT_FIELDS,
            d_id: 7,
            ..FsDiskQuota::default()
        };
        assert!(is_project_limit_change(&change, 7));
        assert!(!is_project_limit_change(&change, 8));
        assert!(!is_project_limit_change(
            &FsDiskQuota { d_id: 0, ..change },
            0
        ));
        assert!(!is_project_limit_change(
            &FsDiskQuota {
                d_flags: FS_PROJ_QUOTA << 1,
                ..change
            },
            7
        ));
        assert!(!is_project_limit_change(
            &FsDiskQuota {
                d_version: FS_DQUOT_VERSION + 1,
                ..change
            },
            7
        ));
        // FS_DQ_BCOUNT, which would rewrite the usage of the project.
        assert!(!is_project_limit_change(
            &FsDiskQuota {
                d_fieldmask: PROJECT_LIMIT_FIELDS | (1 << 6),
                ..change
            },
            7
        ));
    }

    #[test]
    fn the_client_reports_a_refused_helper() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        write_greeting(&mut server, Some("no CAP_SYS_ADMIN"), 0).unwrap();
        let_assert!(Err(error) = read_greeting(&mut client, 0));
        assert!(
            error
                .to_string()
                .contains("refused to start: no CAP_SYS_ADMIN")
        );
    }

    #[test]
    fn the_client_refuses_a_helper_of_another_filesystem() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        write_greeting(&mut server, None, 7).unwrap();
        let_assert!(Err(error) = read_greeting(&mut client, 8));
        assert!(error.to_string().contains("another filesystem"));
    }

    #[test]
    fn the_client_refuses_another_protocol() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        server.write_all(b"GQH2\0\0\0\0\0\0\0\0\0\0\0").unwrap();
        let_assert!(Err(error) = read_greeting(&mut client, 0));
        assert!(error.to_string().contains("another protocol version"));
    }

    #[test]
    fn the_helper_answers_calls_and_returns_their_errors() {
        // A directory that is not on XFS: every quotactl_fd call fails, and the helper gives the
        // error of the kernel back instead of closing the connection.
        let directory = tempfile::tempdir().unwrap();
        let root = File::open(directory.path()).unwrap();
        let (client, server) = UnixStream::pair().unwrap();
        let server_thread = std::thread::spawn(move || serve(&root, server));
        let helper = XfsQuotaHelper::started(client, 0);

        let mut quota = FsDiskQuota::default();
        let_assert!(Err(error) = helper.get_project_quota(1, &mut quota));
        assert!(error.raw_os_error().is_some());
        let mut state = FsQuotaStatV::default();
        let_assert!(Err(_) = helper.get_quota_state(&mut state));

        let refused = FsDiskQuota {
            d_id: 2,
            ..FsDiskQuota::default()
        };
        let_assert!(Err(error) = helper.set_project_quota_record(1, &refused));
        assert!(error.raw_os_error() == Some(libc::EINVAL));

        drop(helper);
        server_thread.join().unwrap();
    }

    #[test]
    fn a_started_helper_that_is_gone_fails_its_calls() {
        let (client, server) = UnixStream::pair().unwrap();
        drop(server);
        let helper = XfsQuotaHelper::started(client, 0);
        let mut quota = FsDiskQuota::default();
        let_assert!(Err(error) = helper.get_project_quota(1, &mut quota));
        assert!(
            error
                .to_string()
                .contains("lost the connection to the XFS quota helper")
        );
    }

    #[test]
    fn a_listening_helper_serves_a_client_of_its_own_user() {
        // The root is not on XFS, so the call reaches the kernel and fails there: the failure
        // comes back as the error of the call, which proves the connection, the greeting and the
        // peer check worked.
        let directory = tempfile::tempdir().unwrap();
        let root = File::open(directory.path()).unwrap();
        let device = root.metadata().unwrap().dev();
        let socket = directory.path().join("helper.sock");
        let listening = socket.clone();
        std::thread::spawn(move || listen(Arc::new(root), device, &listening));
        while !socket.exists() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
        assert!(mode == 0o600);

        let_assert!(Ok(helper) = XfsQuotaHelper::connect(&socket, device));
        let mut quota = FsDiskQuota::default();
        let_assert!(Err(error) = helper.get_project_quota(1, &mut quota));
        assert!(error.raw_os_error().is_some());
        assert!(!error.to_string().contains("XFS quota helper"));

        let_assert!(Err(error) = XfsQuotaHelper::connect(&socket, device + 1));
        assert!(error.to_string().contains("another filesystem"));
    }

    #[test]
    fn helper_arguments_name_a_root_and_one_connection() {
        let parse = |args: &[&str]| HelperArgs::parse(args.iter().map(OsString::from));
        let_assert!(Ok(args) = parse(&["--root", "/xfs", "--listen", "/run/q.sock"]));
        assert!(args.root == Path::new("/xfs"));
        assert!(
            matches!(args.connection, HelperConnection::Listen(path) if path == Path::new("/run/q.sock"))
        );
        let_assert!(Ok(args) = parse(&["--root", "/xfs", "--fd", "3"]));
        assert!(matches!(args.connection, HelperConnection::Descriptor(3)));
        assert!(parse(&["--root", "/xfs"]).is_err());
        assert!(parse(&["--listen", "/run/q.sock"]).is_err());
        assert!(parse(&["--root", "/xfs", "--fd", "x"]).is_err());
        assert!(parse(&["--root"]).is_err());
    }
}
