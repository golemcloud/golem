import base64
import hashlib
import json
import socket
import struct
import sys
import time

mode = sys.argv[1] if len(sys.argv) > 1 else "success"


def read_exact(conn, length):
    data = b""
    while len(data) < length:
        chunk = conn.recv(length - len(data))
        if not chunk:
            raise EOFError("client disconnected")
        data += chunk
    return data


def receive_frame(conn):
    header = read_exact(conn, 2)
    length = header[1] & 127
    if length == 126:
        length = struct.unpack("!H", read_exact(conn, 2))[0]
    elif length == 127:
        length = struct.unpack("!Q", read_exact(conn, 8))[0]
    mask = read_exact(conn, 4)
    data = read_exact(conn, length)
    return bytes(byte ^ mask[i % 4] for i, byte in enumerate(data))


def receive(conn):
    return json.loads(receive_frame(conn))


def send(conn, **message):
    data = json.dumps(dict(version=1, **message), separators=(",", ":")).encode()
    header = (
        bytes([129, len(data)])
        if len(data) < 126
        else b"\x81\x7e" + struct.pack("!H", len(data))
    )
    conn.sendall(header + data)


listener = socket.socket()
listener.bind(("127.0.0.1", 0))
listener.listen()
# The first connection waits for the native build of the client, which can take
# minutes on a loaded machine; every later step has the short timeout.
listener.settimeout(600)
print(listener.getsockname()[1], flush=True)


def connect():
    conn, _ = listener.accept()
    listener.settimeout(15)
    conn.settimeout(15)
    request = b""
    while b"\r\n\r\n" not in request:
        request += read_exact(conn, 1)
    headers = dict(
        line.split(": ", 1)
        for line in request.decode().split("\r\n")[1:]
        if ": " in line
    )
    key = next(
        value for name, value in headers.items() if name.lower() == "sec-websocket-key"
    )
    accept = base64.b64encode(
        hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()
    ).decode()
    conn.sendall(
        (
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: "
            + accept
            + "\r\nSec-WebSocket-Protocol: golem.agent-invocation.v1\r\n\r\n"
        ).encode()
    )
    return conn


def refuse(status):
    conn, _ = listener.accept()
    listener.settimeout(15)
    with conn:
        conn.settimeout(15)
        request = b""
        while b"\r\n\r\n" not in request:
            request += read_exact(conn, 1)
        conn.sendall(
            f"HTTP/1.1 {status} Refused\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".encode()
        )


def terminal():
    listener.settimeout(0.5)
    try:
        conn, _ = listener.accept()
    except TimeoutError:
        listener.close()
        sys.exit(0)
    conn.close()
    raise AssertionError("terminal rejection caused another attachment attempt")


# A closed port: the client gives up after its reconnect window.
if mode == "start-closed":
    listener.close()
    sys.exit(0)

# A peer that accepts TCP connections but never answers the upgrade: each
# attempt is abandoned, and the client gives up after its reconnect window.
if mode == "start-blackhole":
    held = [listener.accept()[0]]
    listener.settimeout(3)
    try:
        while True:
            held.append(listener.accept()[0])
    except TimeoutError:
        pass
    assert len(held) >= 2, f"the client dialed {len(held)} times"
    for conn in held:
        conn.close()
    listener.close()
    sys.exit(0)

def accept_operation(conn, operation, key, mappings):
    send(
        conn,
        type="invocationAccepted",
        attemptId=operation["attemptId"],
        idempotencyKey=key,
        sessionToken="original-session",
        mappings=mappings,
    )


# A peer that accepts every start and resume and drops the connection at once:
# that is no progress, so the reconnects back off and the client gives up after
# its reconnect window.
if mode == "accept-drop":
    connected = []
    key = None
    try:
        while True:
            with connect() as conn:
                connected.append(time.monotonic())
                listener.settimeout(3)
                operation = receive(conn)
                key = key or operation["idempotencyKey"]
                accept_operation(conn, operation, key, [])
    except TimeoutError:
        pass
    assert 3 <= len(connected) <= 6, f"the client connected {len(connected)} times"
    last_wait = connected[-1] - connected[-2]
    assert last_wait >= 0.15, f"the last reconnect waited only {last_wait}s"
    listener.close()
    sys.exit(0)

# Accepted connections that stay up keep the call alive past the reconnect
# window, and each loss reconnects at once.
if mode == "stable-reset":
    output = [{"direction": "output", "channel": 2, "streamToken": "output"}]
    with connect() as conn:
        start = receive(conn)
        key = start["idempotencyKey"]
        accept_operation(conn, start, key, [])
        send(
            conn,
            type="invocationResult",
            mappings=output,
            result={
                "kind": "value",
                "graph": {
                    "root": {
                        "kind": "stream",
                        "value": {"inner": {"kind": "u8", "value": {}}},
                    }
                },
                "value": {"kind": "stream", "value": {"streamToken": "output"}},
            },
        )
        send(
            conn,
            type="outputStreamItem",
            channel=2,
            sequence="0",
            cursorToken="cursor-1",
            mappings=[],
            value={"kind": "u8", "value": 42},
        )
        signal, _ = listener.accept()
        with signal:
            signal.settimeout(15)
            request = b""
            while b"\r\n\r\n" not in request:
                request += read_exact(signal, 1)
            signal.sendall(
                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
        time.sleep(0.5)
    started = time.monotonic()
    for _ in range(3):
        lost = time.monotonic()
        with connect() as conn:
            wait = time.monotonic() - lost
            assert wait < 0.1, f"a reconnect after a stable connection waited {wait}s"
            resume = receive(conn)
            assert resume["operation"] == "resume"
            accept_operation(conn, resume, key, output)
            time.sleep(0.5)
    lost = time.monotonic()
    with connect() as conn:
        wait = time.monotonic() - lost
        assert wait < 0.1, f"a reconnect after a stable connection waited {wait}s"
        resume = receive(conn)
        accept_operation(conn, resume, key, output)
        send(
            conn,
            type="outputStreamItem",
            channel=2,
            sequence="1",
            cursorToken="cursor-2",
            mappings=[],
            value={"kind": "u8", "value": 43},
        )
        send(
            conn,
            type="outputStreamEnd",
            channel=2,
            sequence="2",
            cursorToken="cursor-end",
            outcome={"kind": "ok"},
        )
        send(conn, type="invocationFinished", outcome={"kind": "success"})
    assert time.monotonic() - started >= 1.0
    listener.close()
    sys.exit(0)

# 429 and 5xx on the upgrade are retried.
if mode == "start-503":
    refuse(503)
    refuse(429)
if mode == "start-no-limit":
    for _ in range(5):
        refuse(503)

# A retryable rejection of the start is retried as a new attempt of the same
# invocation.
first_start = None
if mode == "start-retryable":
    with connect() as conn:
        first_start = receive(conn)
        send(
            conn,
            type="invocationRejected",
            attemptId=first_start["attemptId"],
            code="routing-miss",
            message="moving",
            retryable=True,
        )

with connect() as conn:
    start = receive(conn)
    assert start["type"] == "invocationStart"
    if first_start is not None:
        assert start["attemptId"] != first_start["attemptId"]
        assert start["idempotencyKey"] == first_start["idempotencyKey"]
        assert start["methodParameters"] == first_start["methodParameters"]
    if mode == "initial":
        send(
            conn,
            type="invocationRejected",
            attemptId=start["attemptId"],
            code="invalid-attachment-state",
            message="terminal rejection",
            retryable=False,
        )
        terminal()
    send(
        conn,
        type="invocationAccepted",
        attemptId=start["attemptId"],
        idempotencyKey=start["idempotencyKey"],
        sessionToken="original-session",
        mappings=[],
    )
    send(
        conn,
        type="invocationResult",
        mappings=[{"direction": "output", "channel": 2, "streamToken": "output"}],
        result={
            "kind": "value",
            "graph": {
                "root": {
                    "kind": "stream",
                    "value": {"inner": {"kind": "u8", "value": {}}},
                }
            },
            "value": {"kind": "stream", "value": {"streamToken": "output"}},
        },
    )
    send(
        conn,
        type="outputStreamItem",
        channel=2,
        sequence="0",
        cursorToken="cursor-1",
        mappings=[],
        value={"kind": "u8", "value": 42},
    )
    # The client signals only after consuming the item and storing its cursor.
    signal, _ = listener.accept()
    with signal:
        signal.settimeout(15)
        request = b""
        while b"\r\n\r\n" not in request:
            request += read_exact(signal, 1)
        assert request.startswith(b"GET /consumed ")
        signal.sendall(
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
    # The accepted connection outlives the reconnect window and the stable
    # time; the window starts again when it is lost.
    if mode == "window-reset":
        time.sleep(1.5)

# Past acceptance, a server that stays away makes the call unreachable after
# the window.
if mode == "resume-unreachable":
    listener.close()
    sys.exit(0)

# A retryable rejection of a resume is retried as a new resume attempt.
rejected_resume = None
if mode == "resume-routing-miss":
    with connect() as conn:
        rejected_resume = json.loads(receive_frame(conn))
        assert rejected_resume["operation"] == "resume"
        send(
            conn,
            type="invocationRejected",
            attemptId=rejected_resume["attemptId"],
            code="routing-miss",
            message="moving",
            retryable=True,
        )

# Losing a response must retry the exact frozen resume descriptor.
with connect() as conn:
    frozen_resume = receive_frame(conn)
    resume = json.loads(frozen_resume)
    assert resume["operation"] == "resume"
    if rejected_resume is not None:
        assert resume["attemptId"] != rejected_resume["attemptId"]
    assert resume["sessionToken"] == "original-session"
    assert resume["outputCursors"] == ["cursor-1"]

with connect() as conn:
    assert receive_frame(conn) == frozen_resume
    send(
        conn,
        type="invocationRejected",
        attemptId="wrong-attempt" if mode == "mismatch" else resume["attemptId"],
        code="token-invalid" if mode == "other" else "invalid-attachment-state",
        message="terminal rejection",
        retryable=mode == "resume-retryable",
    )
    if mode in ("mismatch", "other"):
        terminal()

with connect() as conn:
    frozen_takeover = receive_frame(conn)
    takeover = json.loads(frozen_takeover)
    assert takeover == dict(
        resume, operation="takeover", attemptId=takeover["attemptId"]
    )
    assert takeover["attemptId"] != resume["attemptId"]
    if mode in ("second", "second-retryable", "takeover-mismatch"):
        send(
            conn,
            type="invocationRejected",
            attemptId=(
                "wrong-attempt"
                if mode == "takeover-mismatch"
                else takeover["attemptId"]
            ),
            code="invalid-attachment-state",
            message="terminal rejection",
            retryable=mode == "second-retryable",
        )
        terminal()

# Losing a takeover response also retries its exact descriptor.
with connect() as conn:
    assert receive_frame(conn) == frozen_takeover
    send(
        conn,
        type="invocationAccepted",
        attemptId=takeover["attemptId"],
        idempotencyKey=start["idempotencyKey"],
        sessionToken="new-session",
        mappings=[{"direction": "output", "channel": 3, "streamToken": "output"}],
    )
    send(
        conn,
        type="outputStreamItem",
        channel=3,
        sequence="1",
        cursorToken="cursor-2",
        mappings=[],
        value={"kind": "u8", "value": 43},
    )
    send(
        conn,
        type="outputStreamEnd",
        channel=3,
        sequence="2",
        cursorToken="cursor-end",
        outcome={"kind": "ok"},
    )
    send(conn, type="invocationFinished", outcome={"kind": "success"})
listener.close()
