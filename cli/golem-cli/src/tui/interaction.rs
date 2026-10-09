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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InteractionMode {
    Normal,
    Leader,
    Layout,
    Jobs,
    QuitBlocked,
    Palette,
    ContextPicker,
    ContextSwitchConfirm,
    Help,
    AgentFilter,
    AgentDatasetFilter,
    AgentColumns,
    MetricsRange,
    CommandForm,
    SessionForm,
    TerminalInput,
    TerminalScrollback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ModeStack {
    modes: Vec<InteractionMode>,
}

impl Default for ModeStack {
    fn default() -> Self {
        Self {
            modes: vec![InteractionMode::Normal],
        }
    }
}

impl ModeStack {
    pub(super) fn current(&self) -> InteractionMode {
        self.modes
            .last()
            .copied()
            .unwrap_or(InteractionMode::Normal)
    }

    pub(super) fn previous(&self) -> Option<InteractionMode> {
        self.modes.iter().rev().nth(1).copied()
    }

    pub(super) fn replace(&mut self, mode: InteractionMode) {
        if let Some(current) = self.modes.last_mut() {
            *current = mode;
        } else {
            self.modes.push(mode);
        }
    }

    pub(super) fn push(&mut self, mode: InteractionMode) {
        if self.current() != mode {
            self.modes.push(mode);
        }
    }

    pub(super) fn pop_or(&mut self, fallback: InteractionMode) -> InteractionMode {
        if self.modes.len() > 1 {
            self.modes.pop();
        } else {
            self.replace(fallback);
        }
        self.current()
    }

    pub(super) fn reset(&mut self, mode: InteractionMode) {
        self.modes.clear();
        self.modes.push(mode);
    }

    #[cfg(test)]
    pub(super) fn contains(&self, mode: InteractionMode) -> bool {
        self.modes.contains(&mode)
    }
}

impl PartialEq<InteractionMode> for ModeStack {
    fn eq(&self, other: &InteractionMode) -> bool {
        self.current() == *other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn nested_modes_restore_the_previous_owner() {
        let mut modes = ModeStack::default();
        modes.replace(InteractionMode::AgentFilter);
        modes.push(InteractionMode::Palette);
        modes.push(InteractionMode::Leader);

        assert_eq!(modes.current(), InteractionMode::Leader);
        assert_eq!(
            modes.pop_or(InteractionMode::Normal),
            InteractionMode::Palette
        );
        assert_eq!(
            modes.pop_or(InteractionMode::Normal),
            InteractionMode::AgentFilter
        );
    }

    #[test]
    fn reset_discards_transient_return_history() {
        let mut modes = ModeStack::default();
        modes.push(InteractionMode::Palette);
        modes.push(InteractionMode::Leader);

        modes.reset(InteractionMode::TerminalInput);

        assert_eq!(modes.current(), InteractionMode::TerminalInput);
        assert!(!modes.contains(InteractionMode::Palette));
    }
}
