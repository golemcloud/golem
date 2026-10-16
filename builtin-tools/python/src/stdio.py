import io
import sys


class Input(io.RawIOBase):
    def readable(self):
        return True

    def readinto(self, buffer):
        self._checkClosed()
        data = read(min(len(buffer), 65536))
        buffer[:len(data)] = data
        return len(data)


class Output(io.RawIOBase):
    def __init__(self, channel):
        super().__init__()
        self.channel = channel

    def writable(self):
        return True

    def write(self, data):
        self._checkClosed()
        data = bytes(data)
        write(self.channel, data)
        return len(data)


sys.stdin = sys.__stdin__ = io.TextIOWrapper(
    io.BufferedReader(Input()), encoding='utf-8', errors='surrogateescape'
)
out = Output(1)
err = Output(2)
if not unbuffered:
    out = io.BufferedWriter(out)
    err = io.BufferedWriter(err)
sys.stdout = sys.__stdout__ = io.TextIOWrapper(
    out, encoding='utf-8', errors='surrogateescape', write_through=unbuffered
)
sys.stderr = sys.__stderr__ = io.TextIOWrapper(
    err, encoding='utf-8', errors='backslashreplace',
    line_buffering=True, write_through=unbuffered
)
