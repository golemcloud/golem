const std = @import("std");
const Builder = std.build.Builder;
const CrossTarget = std.zig.CrossTarget;

pub fn build(b: *Builder) !void {
    const optimize = b.standardOptimizeOption(.{
        .preferred_optimize_mode = .ReleaseSmall,
    });

    const bindgen = b.addSystemCommand(&.{ "wit-bindgen", "c", "--autodrop-borrows", "yes", "./wit", "--out-dir", "src/bindings" });

    const wasm = b.addExecutable(.{ .name = "main", .root_source_file = .{ .path = "src/main.zig" }, .target = .{
        .cpu_arch = .wasm32,
        .os_tag = .wasi,
    }, .optimize = optimize });

    const binding_root = b.pathFromRoot("src/bindings");
    var binding_root_dir = try std.fs.cwd().openIterableDir(binding_root, .{});
    defer binding_root_dir.close();
    var it = try binding_root_dir.walk(b.allocator);
    while (try it.next()) |entry| {
        switch (entry.kind) {
            .file => {
                const path = b.pathJoin(&.{ binding_root, entry.path });
                if (std.mem.endsWith(u8, entry.basename, ".c")) {
                    wasm.addCSourceFile(.{ .file = .{ .path = path }, .flags = &.{} });
                } else if (std.mem.endsWith(u8, entry.basename, ".o")) {
                    wasm.addObjectFile(.{ .path = path });
                }
            },
            else => continue,
        }
    }

    wasm.addIncludePath(.{ .path = binding_root });
    wasm.linkLibC();

    wasm.step.dependOn(&bindgen.step);

    const adapter = b.option([]const u8, "adapter", "Path to the Golem Tier1 WASI adapter") orelse "adapters/tier1/wasi_snapshot_preview1.wasm";
    const out = try std.fmt.allocPrint(b.allocator, "zig-out/bin/{s}", .{wasm.out_filename});
    const component = b.addSystemCommand(&.{ "wasm-tools", "component", "new", out, "-o", "zig-out/bin/component.wasm", "--adapt", adapter });
    component.step.dependOn(&wasm.step);

    b.installArtifact(wasm);
    b.getInstallStep().dependOn(&component.step);
}
