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

//! Pins `signal_unreaped_child`'s process-supervision behavior. It lives here rather than as a
//! `--lib` unit test in `spawned.rs` because it needs a real child process to signal, and unit
//! tests must never spawn external processes (AGENTS.md); `cargo make unit-tests` runs
//! `--workspace --lib` and would otherwise pick it up.

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
