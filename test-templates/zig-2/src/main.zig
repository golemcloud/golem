const std = @import("std");

var gpa = std.heap.GeneralPurposeAllocator(.{}){};

const CommandTag = enum { add, get };

const Command = union(CommandTag) { add: i32, get: void };

pub fn main() anyerror!void {
    const stdin = std.io.getStdIn().reader();
    const stdout = std.io.getStdOut().writer();
    const stderr = std.io.getStdErr().writer();

    var buf: [100]u8 = undefined;
    var state: i32 = 0;

    // Reading parameters from the standard input
    while(true) {
        if (try stdin.readUntilDelimiterOrEof(buf[0..], '\n')) |input| {
            if (std.json.parseFromSlice(Command, gpa.allocator(), input, .{})) |parsed| {
                defer parsed.deinit();
                switch (parsed.value) {
                    CommandTag.add => |value| {
                        state += value;
                        try stdout.print("{{}}\n", .{});
                    },
                    CommandTag.get => {
                        try stdout.print("{}\n", .{state});
                    },
                }
            } else |err| {
                try stderr.print("failed to parse command: {}", .{err});
            }
        }
    }
}
