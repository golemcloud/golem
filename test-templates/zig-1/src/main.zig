const std = @import("std");

var gpa = std.heap.GeneralPurposeAllocator(.{}){};

pub fn main() anyerror!void {
    const stdin = std.io.getStdIn().reader();
    const stdout = std.io.getStdOut().writer();
    const stderr = std.io.getStdErr().writer();

    var buf: [100]u8 = undefined;

    // Reading parameters from the standard input
    if (try stdin.readUntilDelimiterOrEof(buf[0..], '\n')) |input| {
        if (std.fmt.parseInt(u64, input, 10)) |value| {
            try stderr.print("Returning 2 * {}", .{value});

            const result = 2 * value;
            // Writing the result to the standard output
            try stdout.print("{}", .{result});
        } else |err| {
            try stderr.print("Input {s} is not a number: {}", .{ input, err });
        }
    }
}
