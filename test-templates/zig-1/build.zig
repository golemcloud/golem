const std = @import("std");
const Builder = std.build.Builder;
const CrossTarget = std.zig.CrossTarget;

pub fn build(b: *Builder) void {
    const optimize = b.standardOptimizeOption(.{
        .preferred_optimize_mode = .ReleaseSmall,
    });

    const exe = b.addExecutable(.{
        .name = "main",
        .root_source_file = .{ .path = "src/main.zig" },
        .target = .{
            .cpu_arch = .wasm32,
            .os_tag = .wasi,
        },
        .optimize = optimize
    });
    b.installArtifact(exe);
}
