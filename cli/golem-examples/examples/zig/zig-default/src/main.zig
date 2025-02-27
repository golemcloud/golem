const std = @import("std");

var gpa = std.heap.GeneralPurposeAllocator(.{}){};

const CommandTag = enum { add, get };

const Command = union(CommandTag) { add: i32, get: void };

var state: u64 = 0;

export fn exports_pack_name_api_add(value: u64) void {
    const stdout = std.io.getStdOut().writer();
    stdout.print("Adding {} to state\n", .{value}) catch unreachable;
    state += value;
}

export fn exports_pack_name_api_get() u64 {
    return state;
}

pub fn main() anyerror!void {}
