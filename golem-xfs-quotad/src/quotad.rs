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

use anyhow::{Context, anyhow, bail};
use std::ffi::OsString;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The documentation of the deployment of `xfs-quotad`.
pub const XFS_QUOTAD_DOCS: &str =
    "https://learn.golem.cloud/operate/filesystem_snapshots#run-xfs-quotad";

const XFS_SUPER_MAGIC: u64 = 0x5846_5342;
const XQM_PRJQUOTA: u32 = 2;
const Q_XGETQUOTA: u32 = (b'X' as u32) << 8 | 3;
const Q_XSETQLIM: u32 = (b'X' as u32) << 8 | 4;
const Q_XGETQSTATV: u32 = (b'X' as u32) << 8 | 8;
pub const FS_DQUOT_VERSION: i8 = 1;
pub const FS_QSTATV_VERSION1: i8 = 1;
pub const FS_PROJ_QUOTA: i8 = 1 << 1;
pub const FS_DQ_ISOFT: u16 = 1 << 0;
pub const FS_DQ_IHARD: u16 = 1 << 1;
pub const FS_DQ_BSOFT: u16 = 1 << 2;
pub const FS_DQ_BHARD: u16 = 1 << 3;
pub const FS_DQ_RTBSOFT: u16 = 1 << 4;
pub const FS_DQ_RTBHARD: u16 = 1 << 5;
/// Every limit field of a project quota record.
pub const PROJECT_LIMIT_FIELDS: u16 =
    FS_DQ_ISOFT | FS_DQ_IHARD | FS_DQ_BSOFT | FS_DQ_BHARD | FS_DQ_RTBSOFT | FS_DQ_RTBHARD;
/// The limit fields of a project quota record for the data device.
pub const PROJECT_DATA_LIMIT_FIELDS: u16 = FS_DQ_ISOFT | FS_DQ_IHARD | FS_DQ_BSOFT | FS_DQ_BHARD;

/// `struct fs_disk_quota` of `linux/dqblk_xfs.h`: the quota record of one project.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FsDiskQuota {
    pub d_version: i8,
    pub d_flags: i8,
    pub d_fieldmask: u16,
    pub d_id: u32,
    pub d_blk_hardlimit: u64,
    pub d_blk_softlimit: u64,
    pub d_ino_hardlimit: u64,
    pub d_ino_softlimit: u64,
    pub d_bcount: u64,
    pub d_icount: u64,
    pub d_itimer: i32,
    pub d_btimer: i32,
    pub d_iwarns: u16,
    pub d_bwarns: u16,
    pub d_itimer_hi: i8,
    pub d_btimer_hi: i8,
    pub d_rtbtimer_hi: i8,
    pub d_padding2: i8,
    pub d_rtb_hardlimit: u64,
    pub d_rtb_softlimit: u64,
    pub d_rtbcount: u64,
    pub d_rtbtimer: i32,
    pub d_rtbwarns: u16,
    pub d_padding3: i16,
    pub d_padding4: [i8; 8],
}

/// `struct fs_qfilestatv` of `linux/dqblk_xfs.h`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FsQuotaFileStatV {
    pub qfs_ino: u64,
    pub qfs_nblks: u64,
    pub qfs_nextents: u32,
    pub qfs_pad: u32,
}

/// `struct fs_quota_statv` of `linux/dqblk_xfs.h`: the quota state of a volume.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FsQuotaStatV {
    pub qs_version: i8,
    pub qs_pad1: u8,
    pub qs_flags: u16,
    pub qs_incoredqs: u32,
    pub qs_uquota: FsQuotaFileStatV,
    pub qs_gquota: FsQuotaFileStatV,
    pub qs_pquota: FsQuotaFileStatV,
    pub qs_btimelimit: i32,
    pub qs_itimelimit: i32,
    pub qs_rtbtimelimit: i32,
    pub qs_bwarnlimit: u16,
    pub qs_iwarnlimit: u16,
    pub qs_rtbwarnlimit: u16,
    pub qs_pad3: u16,
    pub qs_pad4: u32,
    pub qs_pad2: [u64; 7],
}

const GREETING_MAGIC: [u8; 4] = *b"GQH1";
const GREETING_READY: u8 = 0;
const GREETING_REFUSED: u8 = 1;

/// The most connections that xfs-quotad serves at once. The executor holds one; the limit stops
/// a client from exhausting the threads or the memory of xfs-quotad.
const MAX_CONNECTIONS: usize = 8;

/// How long a call of a client may take in all, from a connection that the call makes, through
/// the greeting, to the end of the answer; and how long the server waits for a client to take an
/// answer. Each call is one quota system call, which takes microseconds, so only a stalled process
/// misses it.
const CALL_DEADLINE: Duration = Duration::from_secs(5);

/// The status of a response to a limit change that xfs-quotad did not apply, because the change
/// came on a connection that is not the newest; see [`Fence`]. Every other status is 0 or an
/// errno, which is positive.
const STATUS_FENCED: i32 = -1;

const OPERATION_QUOTA_STATE: u8 = 1;
const OPERATION_PROJECT_QUOTA: u8 = 2;
const OPERATION_SET_PROJECT_LIMITS: u8 = 3;

const QUOTA_BYTES: usize = 112;
const QUOTA_STATE_BYTES: usize = 160;
const REQUEST_BYTES: usize = 1 + 4 + QUOTA_BYTES;
const RESPONSE_BYTES: usize = 4 + QUOTA_STATE_BYTES;

/// A connection to xfs-quotad of one XFS root.
pub struct XfsQuotadClient {
    connection: Mutex<Connection>,
}

struct Connection {
    /// The socket of xfs-quotad. A connection without one cannot be made again.
    socket: Option<PathBuf>,
    /// The device of the XFS root that xfs-quotad must serve.
    device: u64,
    /// The time that one call may take in all.
    call_deadline: Duration,
    stream: Option<UnixStream>,
}

impl XfsQuotadClient {
    /// Connects to xfs-quotad that listens on `socket`, and checks that it serves the filesystem
    /// with the device `device`.
    pub fn connect(socket: &Path, device: u64) -> std::io::Result<Self> {
        let stream = connect_socket(socket, device, Instant::now() + CALL_DEADLINE)?;
        Ok(Self {
            connection: Mutex::new(Connection {
                socket: Some(socket.to_path_buf()),
                device,
                call_deadline: CALL_DEADLINE,
                stream: Some(stream),
            }),
        })
    }

    pub fn get_quota_state(&self, state: &mut FsQuotaStatV) -> std::io::Result<()> {
        let mut request = [0u8; REQUEST_BYTES];
        request[0] = OPERATION_QUOTA_STATE;
        let response = self.call(&request)?;
        *state = decode_quota_state(&response[..QUOTA_STATE_BYTES]);
        Ok(())
    }

    pub fn get_project_quota(
        &self,
        project_id: u32,
        quota: &mut FsDiskQuota,
    ) -> std::io::Result<()> {
        let request = encode_request(OPERATION_PROJECT_QUOTA, project_id, &FsDiskQuota::default());
        let response = self.call(&request)?;
        *quota = decode_quota(&response[..QUOTA_BYTES]);
        Ok(())
    }

    pub fn set_project_quota_record(
        &self,
        project_id: u32,
        quota: &FsDiskQuota,
    ) -> std::io::Result<()> {
        let request = encode_request(OPERATION_SET_PROJECT_LIMITS, project_id, quota);
        self.call(&request).map(|_| ())
    }

    /// Sends `request` and gives the payload of the response, or the error of the call. The whole
    /// call, with a connection that it makes, has one deadline. A connection that fails is closed,
    /// so that a late answer is never read as the answer of the next request. When the connection
    /// broke or xfs-quotad fenced it, and the deadline has not passed, the call connects again and
    /// sends the request once more; every call is idempotent, so a repeat cannot apply a change
    /// twice. The new connection fences the old one, so a request on the old connection that
    /// xfs-quotad reads late cannot apply a change after the requests on the new one.
    fn call(&self, request: &[u8; REQUEST_BYTES]) -> std::io::Result<[u8; QUOTA_STATE_BYTES]> {
        let mut connection = self
            .connection
            .lock()
            .expect("XFS quota server lock poisoned");
        let deadline = Instant::now() + connection.call_deadline;
        match connection.exchange(request, deadline) {
            Ok(result) => return result,
            Err(Transport(error)) => {
                connection.stream = None;
                let Some(socket) = connection.socket.clone() else {
                    return Err(server_lost(error));
                };
                if error.kind() == ErrorKind::TimedOut {
                    return Err(server_lost(error));
                }
                connection.stream = Some(connect_socket(&socket, connection.device, deadline)?);
            }
        }
        match connection.exchange(request, deadline) {
            Ok(result) => result,
            Err(Transport(error)) => {
                connection.stream = None;
                Err(server_lost(error))
            }
        }
    }
}

#[cfg(test)]
impl XfsQuotadClient {
    /// Wraps a connected `stream` whose greeting was read already. It is not connected again.
    fn over(stream: UnixStream, device: u64, call_deadline: Duration) -> Self {
        Self {
            connection: Mutex::new(Connection {
                socket: None,
                device,
                call_deadline,
                stream: Some(stream),
            }),
        }
    }
}

/// A failure to reach xfs-quotad, as opposed to an error of the call that xfs-quotad made.
struct Transport(std::io::Error);

impl Connection {
    fn exchange(
        &mut self,
        request: &[u8; REQUEST_BYTES],
        deadline: Instant,
    ) -> Result<std::io::Result<[u8; QUOTA_STATE_BYTES]>, Transport> {
        let call_deadline = self.call_deadline;
        let transport = |error: std::io::Error| {
            Transport(if error.kind() == ErrorKind::TimedOut {
                std::io::Error::new(
                    ErrorKind::TimedOut,
                    format!("xfs-quotad did not answer within {call_deadline:?}"),
                )
            } else {
                error
            })
        };
        let stream = match &mut self.stream {
            Some(stream) => stream,
            None => {
                return Err(Transport(std::io::Error::new(
                    ErrorKind::NotConnected,
                    "xfs-quotad connection is closed",
                )));
            }
        };
        write_by(stream, request, deadline).map_err(transport)?;
        let mut response = [0u8; RESPONSE_BYTES];
        read_by(stream, &mut response, deadline).map_err(transport)?;
        let status = i32::from_le_bytes(response[..4].try_into().unwrap());
        let mut payload = [0u8; QUOTA_STATE_BYTES];
        payload.copy_from_slice(&response[4..]);
        match status {
            0 => Ok(Ok(payload)),
            STATUS_FENCED => Err(Transport(std::io::Error::new(
                ErrorKind::ConnectionReset,
                "xfs-quotad has a newer connection, and changes limits only on that one",
            ))),
            errno => Ok(Err(std::io::Error::from_raw_os_error(errno))),
        }
    }
}

/// The time left until `deadline`, or a `TimedOut` error when it has passed.
fn time_left(deadline: Instant) -> std::io::Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(ErrorKind::TimedOut.into())
    } else {
        Ok(left)
    }
}

/// A `TimedOut` error in place of the `WouldBlock` error of a socket timeout.
fn timed_out(error: std::io::Error) -> std::io::Error {
    if error.kind() == ErrorKind::WouldBlock {
        ErrorKind::TimedOut.into()
    } else {
        error
    }
}

/// Reads exactly `buffer.len()` bytes from `stream`, and fails when that does not end by
/// `deadline`, also when every read makes progress.
fn read_by(stream: &mut UnixStream, buffer: &mut [u8], deadline: Instant) -> std::io::Result<()> {
    let mut filled = 0;
    while filled < buffer.len() {
        stream.set_read_timeout(Some(time_left(deadline)?))?;
        match stream.read(&mut buffer[filled..]) {
            Ok(0) => return Err(ErrorKind::UnexpectedEof.into()),
            Ok(read) => filled += read,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(timed_out(error)),
        }
    }
    Ok(())
}

/// Writes all of `buffer` to `stream`, and fails when that does not end by `deadline`.
fn write_by(stream: &mut UnixStream, buffer: &[u8], deadline: Instant) -> std::io::Result<()> {
    let mut written = 0;
    while written < buffer.len() {
        stream.set_write_timeout(Some(time_left(deadline)?))?;
        match stream.write(&buffer[written..]) {
            Ok(0) => return Err(ErrorKind::WriteZero.into()),
            Ok(count) => written += count,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(timed_out(error)),
        }
    }
    Ok(())
}

fn server_lost(error: std::io::Error) -> std::io::Error {
    std::io::Error::new(
        error.kind(),
        format!("lost the connection to xfs-quotad: {error}"),
    )
}

/// Connects to xfs-quotad on `socket` and reads its greeting, by `deadline`. The send timeout
/// also bounds the connect, which waits while the backlog of the socket is full.
fn connect_socket(socket: &Path, device: u64, deadline: Instant) -> std::io::Result<UnixStream> {
    let connect = || -> std::io::Result<UnixStream> {
        let stream = UnixStream::from(rustix::net::socket_with(
            rustix::net::AddressFamily::UNIX,
            rustix::net::SocketType::STREAM,
            rustix::net::SocketFlags::CLOEXEC,
            None,
        )?);
        stream.set_write_timeout(Some(time_left(deadline)?))?;
        rustix::net::connect(&stream, &rustix::net::SocketAddrUnix::new(socket)?)
            .map_err(|errno| timed_out(errno.into()))?;
        Ok(stream)
    };
    let mut stream = connect().map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!(
                "failed to connect to xfs-quotad at {}: {error}. See {XFS_QUOTAD_DOCS}",
                socket.display()
            ),
        )
    })?;
    read_greeting(&mut stream, device, deadline)?;
    Ok(stream)
}

/// Reads the greeting of xfs-quotad on `stream` by `deadline`, and checks that xfs-quotad is
/// ready and serves the filesystem with the device `device`.
fn read_greeting(stream: &mut UnixStream, device: u64, deadline: Instant) -> std::io::Result<()> {
    let mut header = [0u8; 4 + 1 + 8 + 2];
    read_by(stream, &mut header, deadline).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("xfs-quotad sent no greeting: {error}"),
        )
    })?;
    if header[..4] != GREETING_MAGIC {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            "xfs-quotad speaks another version of its protocol than the executor",
        ));
    }
    let status = header[4];
    let server_device = u64::from_le_bytes(header[5..13].try_into().unwrap());
    let message_length = u16::from_le_bytes(header[13..15].try_into().unwrap()) as usize;
    let mut message = vec![0u8; message_length];
    read_by(stream, &mut message, deadline)?;
    let message = String::from_utf8_lossy(&message);
    if status != GREETING_READY {
        return Err(std::io::Error::new(
            ErrorKind::PermissionDenied,
            format!("xfs-quotad refused the connection: {message}"),
        ));
    }
    if server_device != device {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "xfs-quotad serves another filesystem than the executor's XFS root",
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

/// Serves the quota calls for the XFS root `root` on a thread of this process, on a socket in a
/// new temporary directory, and gives the path of the socket. The process must hold
/// `CAP_SYS_ADMIN` itself; this is for a process that runs no guest code, such as a test.
pub fn start_in_process(root: &Path) -> std::io::Result<PathBuf> {
    let root_file = open_root(root).map_err(std::io::Error::other)?;
    let device = root_file.metadata()?.dev();
    let directory = tempfile::Builder::new()
        .prefix("xfs-quotad")
        .tempdir()?
        .keep();
    let socket = directory.join("xfs-quotad.sock");
    let listener = bind(&socket).map_err(std::io::Error::other)?;
    let uid = rustix::process::geteuid();
    std::thread::Builder::new()
        .name("xfs-quotad".to_string())
        .spawn(move || serve_connections(listener, Arc::new(root_file), device, uid))?;
    Ok(socket)
}

// --- The xfs-quotad process ---

/// Runs `xfs-quotad` with the command line arguments `args`, without the program name, until the
/// process stops.
pub fn run(args: impl Iterator<Item = OsString>) -> anyhow::Result<()> {
    let args = Args::parse(args)?;
    require_sys_admin()?;
    let root = open_root(&args.root)?;
    let device = root.metadata()?.dev();
    let listener = bind(&args.socket)?;
    let allowed_uid = match args.allowed_uid {
        Some(uid) => rustix::process::Uid::from_raw(uid),
        None => rustix::process::geteuid(),
    };
    serve_connections(listener, Arc::new(root), device, allowed_uid);
    Ok(())
}

struct Args {
    root: PathBuf,
    socket: PathBuf,
    allowed_uid: Option<u32>,
}

impl Args {
    fn parse(mut args: impl Iterator<Item = OsString>) -> anyhow::Result<Self> {
        let usage = || {
            anyhow!(
                "usage: golem-xfs-quotad --root <XFS root> --listen <socket> [--allow-uid <uid>]"
            )
        };
        let mut root = None;
        let mut socket = None;
        let mut allowed_uid = None;
        while let Some(arg) = args.next() {
            let value = args.next().ok_or_else(usage)?;
            match arg.to_str() {
                Some("--root") => root = Some(PathBuf::from(value)),
                Some("--listen") => socket = Some(PathBuf::from(value)),
                Some("--allow-uid") => {
                    allowed_uid = Some(
                        value
                            .to_str()
                            .and_then(|value| value.parse().ok())
                            .ok_or_else(usage)?,
                    )
                }
                _ => return Err(usage()),
            }
        }
        Ok(Self {
            root: root.ok_or_else(usage)?,
            socket: socket.ok_or_else(usage)?,
            allowed_uid,
        })
    }
}

/// Fails, naming the settings to add, when this process does not hold `CAP_SYS_ADMIN` in its
/// effective set. The process only reads its capabilities; the deployment gives them.
fn require_sys_admin() -> anyhow::Result<()> {
    let current = rustix::thread::capabilities(None)
        .context("failed to read the capabilities of the process")?;
    if current
        .effective
        .contains(rustix::thread::CapabilitySet::SYS_ADMIN)
    {
        Ok(())
    } else {
        Err(missing_sys_admin())
    }
}

fn missing_sys_admin() -> anyhow::Error {
    anyhow!(
        "xfs-quotad needs CAP_SYS_ADMIN, which this process does not hold. The binary of the \
         xfs-quotad image has the file capability cap_sys_admin+ep, which Linux grants only when \
         the container adds the SYS_ADMIN capability (Kubernetes: capabilities.add: [SYS_ADMIN]; \
         Docker: --cap-add SYS_ADMIN) and allows privilege escalation (Kubernetes: \
         allowPrivilegeEscalation: true; Docker: no --security-opt no-new-privileges). Without \
         containers, run it with AmbientCapabilities=CAP_SYS_ADMIN. See {XFS_QUOTAD_DOCS}"
    )
}

/// Opens the XFS root `root`. xfs-quotad does not lock the root: the executor holds that lock.
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

/// Listens on `socket`, in place of a file that an earlier server left there. Any process may
/// open the socket: xfs-quotad checks the uid of each connection itself, and the operator keeps
/// the socket in a directory that only xfs-quotad and the executor share.
fn bind(socket: &Path) -> anyhow::Result<UnixListener> {
    match std::fs::remove_file(socket) {
        Err(error) if error.kind() != ErrorKind::NotFound => {
            return Err(error).with_context(|| format!("failed to remove {}", socket.display()));
        }
        _ => {}
    }
    let listener = UnixListener::bind(socket)
        .with_context(|| format!("failed to listen on {}", socket.display()))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o666))
        .with_context(|| format!("failed to open {} to the executor", socket.display()))?;
    Ok(listener)
}

/// Accepts connections on `listener` from processes of the uid `allowed_uid` only, and serves each
/// on its own thread. While [`MAX_CONNECTIONS`] connections are served, it refuses more.
fn serve_connections(
    listener: UnixListener,
    root: Arc<File>,
    device: u64,
    allowed_uid: rustix::process::Uid,
) {
    let served = Arc::new(AtomicUsize::new(0));
    let fence = Arc::new(Fence::default());
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        if stream.set_write_timeout(Some(CALL_DEADLINE)).is_err() {
            continue;
        }
        match rustix::net::sockopt::socket_peercred(&stream) {
            Ok(peer) if peer.uid == allowed_uid => {}
            _ => {
                let _ = write_greeting(
                    &mut stream,
                    Some("xfs-quotad accepts connections of another uid"),
                    device,
                );
                continue;
            }
        }
        if served.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
            served.fetch_sub(1, Ordering::AcqRel);
            let _ = write_greeting(
                &mut stream,
                Some("xfs-quotad serves the most connections it allows"),
                device,
            );
            continue;
        }
        let root = Arc::clone(&root);
        let fence = Arc::clone(&fence);
        let spawned = std::thread::Builder::new()
            .name("xfs-quotad-connection".to_string())
            .spawn({
                let served = Arc::clone(&served);
                move || {
                    let ticket = fence.enter();
                    if write_greeting(&mut stream, None, device).is_ok() {
                        serve(&root, &fence, ticket, stream);
                    }
                    served.fetch_sub(1, Ordering::AcqRel);
                }
            });
        if spawned.is_err() {
            served.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// Orders the limit changes of all connections. Each connection takes a ticket before its
/// greeting, and xfs-quotad applies a limit change only on the connection with the newest ticket.
/// A client connects again only after it gave up on its connection, so a request on the old
/// connection that xfs-quotad reads late cannot apply a change after the requests on the new one.
#[derive(Default)]
struct Fence {
    newest: Mutex<u64>,
}

impl Fence {
    /// Gives the ticket of a new connection, which fences all older connections.
    fn enter(&self) -> u64 {
        let mut newest = self.newest.lock().expect("xfs-quotad fence lock poisoned");
        *newest += 1;
        *newest
    }

    /// Runs `change` when `ticket` is the newest ticket, and gives `None` otherwise. No connection
    /// takes a ticket while `change` runs, so a change that runs ends before the greeting of a
    /// newer connection.
    fn apply(
        &self,
        ticket: u64,
        change: impl FnOnce() -> std::io::Result<()>,
    ) -> Option<std::io::Result<()>> {
        let newest = self.newest.lock().expect("xfs-quotad fence lock poisoned");
        (*newest == ticket).then(change)
    }
}

/// Answers the calls on `stream`, the connection with the ticket `ticket`, until the client
/// closes it, or does not take an answer within the write deadline of `stream`. A connection may
/// stay idle between calls for any time.
fn serve(root: &File, fence: &Fence, ticket: u64, mut stream: UnixStream) {
    let mut request = [0u8; REQUEST_BYTES];
    while stream.read_exact(&mut request).is_ok() {
        let response = answer(root, fence, ticket, &request);
        if stream.write_all(&response).is_err() {
            return;
        }
    }
}

fn answer(
    root: &File,
    fence: &Fence,
    ticket: u64,
    request: &[u8; REQUEST_BYTES],
) -> [u8; RESPONSE_BYTES] {
    let mut response = [0u8; RESPONSE_BYTES];
    let project_id = u32::from_le_bytes(request[1..5].try_into().unwrap());
    let result = match request[0] {
        OPERATION_QUOTA_STATE => {
            let mut state = FsQuotaStatV {
                qs_version: FS_QSTATV_VERSION1,
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
                let applied = fence.apply(ticket, || {
                    // SAFETY: Q_XSETQLIM reads exactly one `fs_disk_quota` record.
                    unsafe {
                        quotactl(
                            root,
                            Q_XSETQLIM,
                            project_id,
                            std::ptr::from_mut(&mut quota).cast(),
                        )
                    }
                });
                let Some(result) = applied else {
                    response[..4].copy_from_slice(&STATUS_FENCED.to_le_bytes());
                    return response;
                };
                result
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
            qs_version: FS_QSTATV_VERSION1,
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
    fn the_client_reports_a_refused_connection() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        write_greeting(&mut server, Some("no CAP_SYS_ADMIN"), 0).unwrap();
        let_assert!(Err(error) = read_greeting(&mut client, 0, Instant::now() + CALL_DEADLINE));
        assert!(
            error
                .to_string()
                .contains("refused the connection: no CAP_SYS_ADMIN")
        );
    }

    #[test]
    fn the_client_refuses_a_server_of_another_filesystem() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        write_greeting(&mut server, None, 7).unwrap();
        let_assert!(Err(error) = read_greeting(&mut client, 8, Instant::now() + CALL_DEADLINE));
        assert!(error.to_string().contains("another filesystem"));
    }

    #[test]
    fn the_client_refuses_another_protocol() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        server.write_all(b"GQH2\0\0\0\0\0\0\0\0\0\0\0").unwrap();
        let_assert!(Err(error) = read_greeting(&mut client, 0, Instant::now() + CALL_DEADLINE));
        assert!(
            error
                .to_string()
                .contains("another version of its protocol")
        );
    }

    #[test]
    fn the_server_answers_calls_and_returns_their_errors() {
        // A directory that is not on XFS: every quotactl_fd call fails, and xfs-quotad gives the
        // error of the kernel back instead of closing the connection.
        let directory = tempfile::tempdir().unwrap();
        let root = File::open(directory.path()).unwrap();
        let (client, server) = UnixStream::pair().unwrap();
        let server_thread = std::thread::spawn(move || serve(&root, &Fence::default(), 0, server));
        let client = XfsQuotadClient::over(client, 0, CALL_DEADLINE);

        let mut quota = FsDiskQuota::default();
        let_assert!(Err(error) = client.get_project_quota(1, &mut quota));
        assert!(error.raw_os_error().is_some());
        let mut state = FsQuotaStatV::default();
        let_assert!(Err(_) = client.get_quota_state(&mut state));

        let refused = FsDiskQuota {
            d_id: 2,
            ..FsDiskQuota::default()
        };
        let_assert!(Err(error) = client.set_project_quota_record(1, &refused));
        assert!(error.raw_os_error() == Some(libc::EINVAL));

        drop(client);
        server_thread.join().unwrap();
    }

    #[test]
    fn a_server_that_is_gone_fails_its_calls() {
        let (client, server) = UnixStream::pair().unwrap();
        drop(server);
        let client = XfsQuotadClient::over(client, 0, CALL_DEADLINE);
        let mut quota = FsDiskQuota::default();
        let_assert!(Err(error) = client.get_project_quota(1, &mut quota));
        assert!(
            error
                .to_string()
                .contains("lost the connection to xfs-quotad")
        );
    }

    #[test]
    fn a_server_that_does_not_answer_fails_the_call_at_the_deadline() {
        let (client, _silent) = UnixStream::pair().unwrap();
        let client = XfsQuotadClient::over(client, 0, Duration::from_millis(50));
        let start = std::time::Instant::now();
        let mut quota = FsDiskQuota::default();
        let_assert!(Err(error) = client.get_project_quota(1, &mut quota));
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(error.kind() == ErrorKind::TimedOut);
        assert!(error.to_string().contains("did not answer"));

        // The connection is closed, so a late answer cannot be taken for the next call.
        let_assert!(Err(error) = client.get_project_quota(1, &mut quota));
        assert!(error.to_string().contains("connection is closed"));
    }

    #[test]
    fn a_server_that_answers_slowly_fails_the_call_at_the_deadline() {
        // The server sends one byte of the answer every 10 ms. Each read makes progress, so a
        // timeout of each read alone would wait for the whole answer, for more than a second.
        let (client, mut server) = UnixStream::pair().unwrap();
        std::thread::spawn(move || {
            let mut request = [0u8; REQUEST_BYTES];
            server.read_exact(&mut request).unwrap();
            for _ in 0..RESPONSE_BYTES {
                std::thread::sleep(Duration::from_millis(10));
                if server.write_all(&[0]).is_err() {
                    return;
                }
            }
        });
        let client = XfsQuotadClient::over(client, 0, Duration::from_millis(200));
        let start = Instant::now();
        let mut quota = FsDiskQuota::default();
        let_assert!(Err(error) = client.get_project_quota(1, &mut quota));
        assert!(start.elapsed() < Duration::from_millis(1000));
        assert!(error.kind() == ErrorKind::TimedOut);
        assert!(error.to_string().contains("did not answer"));
    }

    fn limit_change(project_id: u32) -> FsDiskQuota {
        FsDiskQuota {
            d_version: FS_DQUOT_VERSION,
            d_flags: FS_PROJ_QUOTA,
            d_fieldmask: FS_DQ_BHARD,
            d_id: project_id,
            ..FsDiskQuota::default()
        }
    }

    #[test]
    fn the_server_changes_limits_only_on_the_newest_connection() {
        let directory = tempfile::tempdir().unwrap();
        let root = File::open(directory.path()).unwrap();
        let fence = Fence::default();
        let old = fence.enter();
        let new = fence.enter();
        let status =
            |response: [u8; RESPONSE_BYTES]| i32::from_le_bytes(response[..4].try_into().unwrap());
        let change = encode_request(OPERATION_SET_PROJECT_LIMITS, 7, &limit_change(7));
        assert!(status(answer(&root, &fence, old, &change)) == STATUS_FENCED);
        // The root is not on XFS, so the kernel refuses the change of the newest connection.
        assert!(status(answer(&root, &fence, new, &change)) > 0);
        // A fenced connection still reads quotas, which changes nothing.
        let read = encode_request(OPERATION_PROJECT_QUOTA, 7, &FsDiskQuota::default());
        assert!(status(answer(&root, &fence, old, &read)) > 0);
    }

    #[test]
    fn a_fenced_client_that_cannot_connect_again_fails_the_change() {
        let directory = tempfile::tempdir().unwrap();
        let root = File::open(directory.path()).unwrap();
        let fence = Arc::new(Fence::default());
        let ticket = fence.enter();
        fence.enter();
        let (client, server) = UnixStream::pair().unwrap();
        std::thread::spawn(move || serve(&root, &fence, ticket, server));
        let client = XfsQuotadClient::over(client, 0, CALL_DEADLINE);
        let_assert!(Err(error) = client.set_project_quota_record(7, &limit_change(7)));
        assert!(error.to_string().contains("has a newer connection"));
    }

    fn start_listening(allowed_uid: rustix::process::Uid) -> (tempfile::TempDir, PathBuf, u64) {
        let directory = tempfile::tempdir().unwrap();
        let root = File::open(directory.path()).unwrap();
        let device = root.metadata().unwrap().dev();
        let socket = directory.path().join("xfs-quotad.sock");
        let listener = bind(&socket).unwrap();
        std::thread::spawn(move || {
            serve_connections(listener, Arc::new(root), device, allowed_uid)
        });
        (directory, socket, device)
    }

    #[test]
    fn a_listening_server_serves_a_client_of_the_allowed_uid() {
        // The root is not on XFS, so the call reaches the kernel and fails there: the failure
        // comes back as the error of the call, which proves the connection, the greeting and the
        // peer check worked.
        let (_directory, socket, device) = start_listening(rustix::process::geteuid());
        let mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
        assert!(mode == 0o666);

        let_assert!(Ok(client) = XfsQuotadClient::connect(&socket, device));
        let mut quota = FsDiskQuota::default();
        let_assert!(Err(error) = client.get_project_quota(1, &mut quota));
        assert!(error.raw_os_error().is_some());
        assert!(!error.to_string().contains("XFS quota server"));

        let_assert!(Err(error) = XfsQuotadClient::connect(&socket, device + 1));
        assert!(error.to_string().contains("another filesystem"));

        // `client` holds one connection; xfs-quotad serves up to MAX_CONNECTIONS at once.
        let held: Vec<XfsQuotadClient> = (1..MAX_CONNECTIONS)
            .map(|_| XfsQuotadClient::connect(&socket, device).unwrap())
            .collect();
        let_assert!(Err(error) = XfsQuotadClient::connect(&socket, device));
        assert!(error.to_string().contains("serves the most connections"));

        // A closed connection frees its place.
        drop(held);
        let connected = (0..200).any(|_| {
            std::thread::sleep(Duration::from_millis(5));
            XfsQuotadClient::connect(&socket, device).is_ok()
        });
        assert!(connected);
    }

    #[test]
    fn a_fenced_client_connects_again_and_sends_the_change_once_more() {
        let (_directory, socket, device) = start_listening(rustix::process::geteuid());
        let_assert!(Ok(fenced) = XfsQuotadClient::connect(&socket, device));
        let_assert!(Ok(_newer) = XfsQuotadClient::connect(&socket, device));
        // The root is not on XFS: an errno of the kernel, and not the fence, fails the change.
        let_assert!(Err(error) = fenced.set_project_quota_record(7, &limit_change(7)));
        assert!(error.raw_os_error().is_some());
    }

    #[test]
    fn a_listening_server_refuses_a_client_of_another_uid() {
        let other = rustix::process::geteuid().as_raw().wrapping_add(1);
        let (_directory, socket, device) = start_listening(rustix::process::Uid::from_raw(other));
        let_assert!(Err(error) = XfsQuotadClient::connect(&socket, device));
        assert!(error.to_string().contains("connections of another uid"));
    }

    #[test]
    fn the_missing_capability_error_names_the_settings() {
        let message = missing_sys_admin().to_string();
        assert!(message.contains("CAP_SYS_ADMIN"));
        assert!(message.contains("capabilities.add: [SYS_ADMIN]"));
        assert!(message.contains("--cap-add SYS_ADMIN"));
        assert!(message.contains("allowPrivilegeEscalation: true"));
        assert!(message.contains("AmbientCapabilities=CAP_SYS_ADMIN"));
        assert!(message.contains(XFS_QUOTAD_DOCS));
    }

    #[test]
    fn arguments_name_a_root_a_socket_and_an_allowed_uid() {
        let parse = |args: &[&str]| Args::parse(args.iter().map(OsString::from));
        let_assert!(Ok(args) = parse(&["--root", "/xfs", "--listen", "/run/q.sock"]));
        assert!(args.root == Path::new("/xfs"));
        assert!(args.socket == Path::new("/run/q.sock"));
        assert!(args.allowed_uid.is_none());
        let_assert!(
            Ok(args) = parse(&[
                "--root",
                "/xfs",
                "--listen",
                "/run/q.sock",
                "--allow-uid",
                "65534"
            ])
        );
        assert!(args.allowed_uid == Some(65534));
        assert!(parse(&["--root", "/xfs"]).is_err());
        assert!(parse(&["--listen", "/run/q.sock"]).is_err());
        assert!(parse(&["--root", "/xfs", "--listen", "/q", "--allow-uid", "x"]).is_err());
        assert!(parse(&["--root", "/xfs", "--fd", "3"]).is_err());
        assert!(parse(&["--root"]).is_err());
    }
}
