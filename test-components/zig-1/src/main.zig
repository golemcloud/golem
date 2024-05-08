const std = @import("std");

var gpa = std.heap.GeneralPurposeAllocator(.{}){};

export fn zig1_run() void {
    const stdin = std.io.getStdIn().reader();
    const stdout = std.io.getStdOut().writer();
    const stderr = std.io.getStdErr().writer();

    var buf: [100]u8 = undefined;

    // Reading parameters from the standard input
    if (stdin.readUntilDelimiterOrEof(buf[0..], '\n') catch unreachable) |input| {
        if (std.fmt.parseInt(u64, input, 10)) |value| {
            stderr.print("Returning 2 * {}", .{value}) catch unreachable;

            const result = 2 * value;
            // Writing the result to the standard output
            stdout.print("{}", .{result}) catch unreachable;
        } else |err| {
            stderr.print("Input {s} is not a number: {}", .{ input, err }) catch unreachable;
        }
    }
}

pub fn main() anyerror!void {
}