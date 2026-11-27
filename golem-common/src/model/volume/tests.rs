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

use super::*;
use test_r::test;

#[test]
fn volume_paths_preserve_names_and_reject_escapes_also_when_deserialized() {
    ["", "corpus/a+b%20c.txt", "é/a.txt"]
        .into_iter()
        .for_each(|text| {
            let path = VolumePath::new(text).unwrap();
            assert_eq!(path.as_str(), text);
            let encoded = serde_json::to_string(&path).unwrap();
            assert_eq!(serde_json::from_str::<VolumePath>(&encoded).unwrap(), path);
        });
    ["/a", "../a", "a/../b", "a/./b", "a//b", "a/", "a\0b"]
        .into_iter()
        .for_each(|text| {
            assert_eq!(VolumePath::new(text), Err(VolumeError::UnrepresentablePath));
            assert!(serde_json::from_value::<VolumePath>(serde_json::json!(text)).is_err());
        });
}

#[test]
fn recorded_volume_results_preserve_paths_sizes_state_and_named_errors() {
    let page = Page {
        entries: [
            Entry {
                path: VolumePath::new("corpus/report").unwrap(),
                node: Node::File { size: 19 },
            },
            Entry {
                path: VolumePath::new("corpus/nested").unwrap(),
                node: Node::Directory,
            },
        ]
        .into(),
        after: Some(Cursor::new("page-7")),
    };
    assert_eq!(
        serde_json::from_value::<Page>(serde_json::to_value(&page).unwrap()).unwrap(),
        page
    );
    let state = FileState::from_reference("\"etag-23\"".into(), Some("version-9".into()));
    let recorded = serde_json::to_value(&state).unwrap();
    assert_eq!(
        serde_json::from_value::<FileState>(recorded).unwrap(),
        state
    );
    assert_ne!(
        state,
        FileState::from_reference("\"etag-24\"".into(), Some("version-9".into()))
    );
    assert_eq!(
        FileState::from_reference("\"etag-23\"".into(), Some("null".into())),
        FileState::from_reference("\"etag-23\"".into(), None)
    );
    let error = VolumeError::Denied {
        action: "s3:GetObjectVersion".into(),
    };
    assert_eq!(
        serde_json::from_value::<VolumeError>(serde_json::to_value(&error).unwrap()).unwrap(),
        error
    );
    assert!(error.to_string().contains("s3:GetObjectVersion"));
    assert!(
        VolumeError::ConnectionRefused
            .to_string()
            .contains("nothing was sent")
    );
}

#[test]
fn volume_debug_hides_reflected_credentials_without_changing_recorded_values() {
    let token = "session-token-reflected-by-store";
    let path = VolumePath::new(token).unwrap();
    let cursor = Cursor::new(token);
    let etag = format!("\"{token}\"");
    let state = FileState::from_reference(etag.clone().into(), Some(token.into()));
    let page = Page {
        entries: [Entry {
            path: path.clone(),
            node: Node::File { size: 7 },
        }]
        .into(),
        after: Some(cursor.clone()),
    };
    [
        format!("{path:?}"),
        format!("{cursor:?}"),
        format!("{state:?}"),
        format!("{page:?}"),
    ]
    .into_iter()
    .for_each(|output| {
        assert!(
            !output.contains(token),
            "Debug exposed a reflected credential"
        );
    });
    assert_eq!(path.as_str(), token);
    assert_eq!(cursor.as_str(), token);
    assert_eq!(state.reference(), (etag.as_str(), Some(token)));
    let recorded = serde_json::to_value(&page).unwrap();
    assert_eq!(recorded["entries"][0]["path"], token);
    assert_eq!(recorded["after"], token);
    assert_eq!(serde_json::from_value::<Page>(recorded).unwrap(), page);
    let recorded = serde_json::to_value(&state).unwrap();
    assert_eq!(recorded["etag"], etag);
    assert_eq!(recorded["version"], token);
}
