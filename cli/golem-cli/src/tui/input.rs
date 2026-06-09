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

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn encode_key_for_pty(key: KeyEvent) -> Option<Vec<u8>> {
    match key.code {
        KeyCode::Char(character)
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
        {
            let mut bytes = [0; 4];
            Some(character.encode_utf8(&mut bytes).as_bytes().to_vec())
        }
        KeyCode::Char(character) if key.modifiers == KeyModifiers::CONTROL => ctrl_char(character),
        KeyCode::Char(character) if key.modifiers == KeyModifiers::ALT => {
            let mut bytes = vec![0x1b];
            let mut encoded = [0; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            Some(bytes)
        }
        KeyCode::Enter => Some(b"\r".to_vec()),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => Some(b"\t".to_vec()),
        KeyCode::BackTab => Some(b"\x1b[Z".to_vec()),
        KeyCode::Esc => Some(b"\x1b".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        _ => None,
    }
}

fn ctrl_char(character: char) -> Option<Vec<u8>> {
    let upper = character.to_ascii_uppercase();
    if upper.is_ascii_uppercase() {
        Some(vec![(upper as u8) - b'@'])
    } else {
        match character {
            '[' => Some(vec![0x1b]),
            '\\' => Some(vec![0x1c]),
            ']' => Some(vec![0x1d]),
            '^' => Some(vec![0x1e]),
            '_' => Some(vec![0x1f]),
            '?' => Some(vec![0x7f]),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use test_r::test;

    #[test]
    fn encodes_printable_characters() {
        assert_eq!(
            encode_key_for_pty(key(KeyCode::Char('a'))),
            Some(b"a".to_vec())
        );
        assert_eq!(
            encode_key_for_pty(key(KeyCode::Char('λ'))),
            Some("λ".as_bytes().to_vec())
        );
    }

    #[test]
    fn encodes_control_letters() {
        assert_eq!(
            encode_key_for_pty(modified_key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(vec![3])
        );
        assert_eq!(
            encode_key_for_pty(modified_key(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            Some(vec![21])
        );
    }

    #[test]
    fn encodes_navigation_keys() {
        assert_eq!(
            encode_key_for_pty(key(KeyCode::Up)),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key_for_pty(key(KeyCode::Down)),
            Some(b"\x1b[B".to_vec())
        );
        assert_eq!(
            encode_key_for_pty(key(KeyCode::Delete)),
            Some(b"\x1b[3~".to_vec())
        );
    }

    #[test]
    fn encodes_alt_characters() {
        assert_eq!(
            encode_key_for_pty(modified_key(KeyCode::Char('f'), KeyModifiers::ALT)),
            Some(b"\x1bf".to_vec())
        );
    }

    fn key(code: KeyCode) -> KeyEvent {
        modified_key(code, KeyModifiers::empty())
    }

    fn modified_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }
}
