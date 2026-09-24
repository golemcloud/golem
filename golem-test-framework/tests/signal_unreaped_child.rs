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

//! Pins `signal_unreaped_child`'s process-supervision behavior.
//!
//! It is worth pinning because the guard it implements has no second line of defence: a reaped
//! child's pid is free for the OS to reuse, so a `kill(2)` on it can signal an unrelated process
//! on the machine running the tests. Nothing else in the suite would notice.
//!
//! It lives here rather than as a `--lib` unit test in `spawned.rs` because it needs a real child
//! process to signal, and unit tests must never spawn external processes (AGENTS.md);
//! `cargo make unit-tests` runs `--workspace --lib` and would otherwise pick it up.
//!
//! Everything here is `#[cfg(unix)]`, because `signal_unreaped_child` is. Windows has no
//! `SIGSTOP`/`SIGCONT`, and neither `std` nor `libc`'s Windows shim can suspend a running process
//! and resume it in place - so the stalled-executor scenario cannot be simulated there at all.
//! The gate is for the compiler rather than the test runner: `libc::SIGSTOP` and `libc::kill` do
//! not exist on Windows, so without it the daily Windows job, which only builds, would fail to
//! compile. It runs no tests, and the sharding suite that uses the pause is Linux-only in CI, so
//! the gate costs no coverage. `SpawnedWorkerExecutor::pause`/`resume` keep `#[cfg(not(unix))]`
//! arms that panic naming the platform, so running the suite there fails with the real reason.

test_r::enable!();

#[cfg(unix)]
mod unix {
    use golem_test_framework::components::worker_executor::spawned::signal_unreaped_child;
    use std::process::{Child, Command};
    use test_r::test;

    /// Kills and reaps the child when the test ends, also when an assertion panics, so a failing
    /// run does not leave a stopped process behind.
    struct KilledOnDrop(Child);

    impl Drop for KilledOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    #[should_panic(expected = "has already exited")]
    fn a_child_that_has_been_reaped_is_never_signalled() {
        let mut child = Command::new("true")
            .spawn()
            .expect("failed to spawn `true`");
        // Records the exit status, exactly as `is_running`'s `try_wait` does for an exited child.
        child.wait().expect("failed to wait for `true`");

        signal_unreaped_child(&mut child, libc::SIGSTOP, "pause `true`");
    }

    #[test]
    fn a_live_child_can_be_stopped_and_continued() {
        let mut child = KilledOnDrop(
            Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("failed to spawn `sleep`"),
        );

        signal_unreaped_child(&mut child.0, libc::SIGSTOP, "pause `sleep`");
        // `try_wait` does not report a stopped child, so continuing it is not refused as exited.
        signal_unreaped_child(&mut child.0, libc::SIGCONT, "resume `sleep`");
    }
}
