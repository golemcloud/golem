import base64
import hashlib
import json
import socket
import struct
import sys

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
listener.settimeout(15)
print(listener.getsockname()[1], flush=True)


def connect():
    conn, _ = listener.accept()
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


def terminal():
    listener.settimeout(0.5)
    try:
        conn, _ = listener.accept()
    except TimeoutError:
        listener.close()
        sys.exit(0)
    conn.close()
    raise AssertionError("terminal rejection caused another attachment attempt")


with connect() as conn:
    start = receive(conn)
    assert start["type"] == "invocationStart"
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

# Losing a response must retry the exact frozen resume descriptor.
with connect() as conn:
    frozen_resume = receive_frame(conn)
    resume = json.loads(frozen_resume)
    assert resume["operation"] == "resume"
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
