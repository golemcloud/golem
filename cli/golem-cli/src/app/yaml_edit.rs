// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::fs::{read_to_string, write_str};
use crate::log::{log_warn_action, LogColorize};
use crate::model::app::{
    AppComponentName, Application, BinaryComponentSource, DependencyType, HttpApiDefinitionName,
};
use anyhow::{anyhow, Context};
use nondestructive::yaml::{Document, Id, MappingMut, Separator, SequenceMut, Value, ValueMut};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct AppYamlEditor<'a> {
    application: &'a Application,
    documents: HashMap<PathBuf, Document>,
}

impl<'a> AppYamlEditor<'a> {
    pub fn new(application: &'a Application) -> Self {
        Self {
            application,
            documents: HashMap::default(),
        }
    }

    pub fn accessed_documents(&self) -> impl Iterator<Item = (&PathBuf, &Document)> {
        self.documents.iter()
    }

    /// Returns true if the dependency was inserted and false on update
    pub fn insert_or_update_dependency(
        &mut self,
        component_name: &AppComponentName,
        target_component_source: &BinaryComponentSource,
        dependency_type: DependencyType,
    ) -> anyhow::Result<bool> {
        let path =
            self.target_document_path_for_dependency(component_name, target_component_source);

        let document = self.document_mut(&path)?;
        let root_value = document.as_mut();
        let mut dependencies = root_value
            .into_mapping_key_insert_missing("dependencies")?
            .into_mapping_key_insert_missing(component_name.as_str())?
            .into_sequence_replace_empty()?;

        let mut dep_type_id = None::<Id>;
        for dep in dependencies.as_ref().iter() {
            let dep = dep.as_mapping().ok_or_else(|| {
                anyhow!(
                    "expected mapping for dependency {} - {}, in {}",
                    component_name.as_str(),
                    target_component_source.to_string(),
                    path.display()
                )
            })?;

            if let Some(target) = dep.get("target") {
                let target_value = target.as_str_with_comments_workaround().ok_or_else(|| {
                    anyhow!(
                        "expected target field for dependency {} - {},  in {}",
                        component_name.as_str(),
                        target_component_source.to_string(),
                        path.display()
                    )
                })?;
                if let BinaryComponentSource::AppComponent { name } = target_component_source {
                    if target_value == name.as_str() {
                        dep_type_id = Some(
                            dep.get("type")
                                .ok_or_else(|| {
                                    anyhow!(
                                        "expected type field for dependency {} - {},  in {}",
                                        component_name.as_str(),
                                        target_component_source.to_string(),
                                        path.display()
                                    )
                                })?
                                .id(),
                        );
                        break;
                    }
                }
            } else if let Some(path_field) = dep.get("path") {
                let path_value = path_field
                    .as_str_with_comments_workaround()
                    .ok_or_else(|| {
                        anyhow!(
                            "expected path field for dependency {} - {},  in {}",
                            component_name.as_str(),
                            target_component_source.to_string(),
                            path.display()
                        )
                    })?;
                if let BinaryComponentSource::LocalFile { path: target_path } =
                    target_component_source
                {
                    if path_value == target_path.to_string_lossy() {
                        dep_type_id = Some(
                            dep.get("type")
                                .ok_or_else(|| {
                                    anyhow!(
                                        "expected type field for dependency {} - {},  in {}",
                                        component_name.as_str(),
                                        target_component_source.to_string(),
                                        path.display()
                                    )
                                })?
                                .id(),
                        );
                        break;
                    }
                }
            } else if let Some(url) = dep.get("url") {
                let url_value = url.as_str_with_comments_workaround().ok_or_else(|| {
                    anyhow!(
                        "expected url field for dependency {} - {},  in {}",
                        component_name.as_str(),
                        target_component_source.to_string(),
                        path.display()
                    )
                })?;
                if let BinaryComponentSource::Url { url: target_url } = target_component_source {
                    if url_value == target_url.to_string() {
                        dep_type_id = Some(
                            dep.get("type")
                                .ok_or_else(|| {
                                    anyhow!(
                                        "expected type field for dependency {} - {},  in {}",
                                        component_name.as_str(),
                                        target_component_source.to_string(),
                                        path.display()
                                    )
                                })?
                                .id(),
                        );
                        break;
                    }
                }
            } else {
                Err(anyhow!(
                    "expected target, path or url field for dependency {} - {},  in {}",
                    component_name.as_str(),
                    target_component_source.to_string(),
                    path.display()
                ))?;
            }
        }

        let insert = match dep_type_id {
            Some(dep_type_id) => {
                document
                    .value_mut(dep_type_id)
                    .set_string(dependency_type.as_str());
                false
            }
            None => {
                // See: field_of_sequence_of_mapping_ident_bug
                let empty_on_start = dependencies.as_ref().is_empty();
                if empty_on_start {
                    dependencies.push(Separator::Auto);
                }

                let mut dep = dependencies.push(Separator::Auto).make_mapping();
                match target_component_source {
                    BinaryComponentSource::AppComponent { name } => {
                        dep.insert_str("target", name.as_str());
                    }
                    BinaryComponentSource::LocalFile { path } => {
                        dep.insert_str("path", path.to_string_lossy());
                    }
                    BinaryComponentSource::Url { url } => {
                        dep.insert_str("url", url);
                    }
                }
                dep.insert_str("type", dependency_type.as_str());

                if empty_on_start {
                    dependencies.remove(0);
                }

                true
            }
        };

        Ok(insert)
    }

    pub fn update_api_definition_version(
        &mut self,
        api_definition_name: &HttpApiDefinitionName,
        version: &str,
    ) -> anyhow::Result<()> {
        let path = self.document_path_for_api_definition(api_definition_name);

        let document = self.document_mut(&path)?;
        let root_value = document.as_mut();

        let mut api_definition = root_value
            .into_mapping_key_insert_missing("httpApi")?
            .into_mapping_key_insert_missing("definitions")?
            .into_mapping_key_insert_missing(api_definition_name.as_str())?
            .into_mapping_mut()
            .ok_or_else(|| {
                anyhow!(
                    "expected mapping for HTTP API definition {} in {}",
                    api_definition_name.as_str(),
                    path.display()
                )
            })?;

        api_definition
            .get_mut("version")
            .ok_or_else(|| {
                anyhow!(
                    "missing version field for HTTP API definition {} in {}",
                    api_definition_name.as_str(),
                    path.display()
                )
            })?
            .set_string(version);

        Ok(())
    }

    fn document_mut(&mut self, path: &Path) -> anyhow::Result<&mut Document> {
        if !self.documents.contains_key(path) {
            self.documents.insert(
                path.to_path_buf(),
                nondestructive::yaml::from_slice(read_to_string(path)?).with_context(|| {
                    anyhow!("Failed to parse {} as yaml document", path.display())
                })?,
            );
        }
        Ok(self.documents.get_mut(path).unwrap())
    }

    fn document_path_for_component(&self, component_name: &AppComponentName) -> PathBuf {
        self.application
            .component_source(component_name)
            .to_path_buf()
    }

    fn document_path_for_api_definition(
        &self,
        api_definition_name: &HttpApiDefinitionName,
    ) -> PathBuf {
        self.application
            .http_api_definition_source(api_definition_name)
    }

    fn existing_document_path_for_dependency(
        &self,
        component_name: &AppComponentName,
        target_component_source: &BinaryComponentSource,
    ) -> Option<PathBuf> {
        match target_component_source {
            BinaryComponentSource::AppComponent { name } => self
                .application
                .dependency_source(component_name, name)
                .map(|path| path.to_path_buf()),
            _ => None,
        }
    }

    fn target_document_path_for_dependency(
        &self,
        component_name: &AppComponentName,
        target_component_source: &BinaryComponentSource,
    ) -> PathBuf {
        match self.existing_document_path_for_dependency(component_name, target_component_source) {
            Some(doc) => doc,
            None => self.document_path_for_component(component_name),
        }
    }

    pub fn update_documents(self) -> anyhow::Result<()> {
        for (path, document) in self.accessed_documents() {
            log_warn_action("Updating", path.log_color_highlight().to_string());
            write_str(path, document.to_string())?;
        }
        Ok(())
    }
}

trait ValueExtensions<'a> {
    fn as_str_with_comments_workaround(&self) -> Option<&str>;
    #[allow(dead_code)]
    fn as_i64_with_comments_workaround(&self) -> Option<i64>;
}

impl<'a> ValueExtensions<'a> for Value<'a> {
    // NOTE: ONLY USE THIS IF THE VALUE CANNOT CONTAIN YAML COMMENTS OR WHITESPACE AS VALID VALUE (e.g. it is validated against it),
    //       see nondestructive_yaml_bugs tests for more info
    fn as_str_with_comments_workaround(&self) -> Option<&str> {
        self.as_str().map(|str_value| match str_value.find('#') {
            Some(idx) => str_value[..idx].trim(),
            None => str_value,
        })
    }

    fn as_i64_with_comments_workaround(&self) -> Option<i64> {
        let as_i64 = self.as_i64();
        if as_i64.is_some() {
            return as_i64;
        }

        if let Some(as_str) = self.as_str_with_comments_workaround() {
            return as_str.parse::<i64>().ok();
        }

        None
    }
}

trait ValueMutExtensions<'a> {
    fn into_mapping_replace_empty(self) -> anyhow::Result<MappingMut<'a>>;

    fn into_mapping_key_insert_missing(self, key: &str) -> anyhow::Result<ValueMut<'a>>;

    fn into_sequence_replace_empty(self) -> anyhow::Result<SequenceMut<'a>>;
}

impl<'a> ValueMutExtensions<'a> for ValueMut<'a> {
    fn into_mapping_replace_empty(self) -> anyhow::Result<MappingMut<'a>> {
        if self.as_ref().as_str() == Some("") {
            Ok(self.make_mapping())
        } else {
            self.into_mapping_mut()
                .ok_or_else(|| anyhow!("expected a mapping"))
        }
    }

    fn into_mapping_key_insert_missing(self, key: &str) -> anyhow::Result<ValueMut<'a>> {
        let mut mapping = self.into_mapping_replace_empty()?;
        {
            let field = mapping.as_ref().get(key);
            let insert = match field {
                Some(value) => value.as_str() == Some(""),
                None => true,
            };
            if insert {
                mapping.insert_str(key, "")
            }
        }
        Ok(mapping.get_into_mut(key).unwrap())
    }

    fn into_sequence_replace_empty(self) -> anyhow::Result<SequenceMut<'a>> {
        if self.as_ref().as_str() == Some("") {
            Ok(self.make_sequence())
        } else {
            self.into_sequence_mut()
                .ok_or_else(|| anyhow!("expected sequence"))
        }
    }
}

#[cfg(test)]
mod tests {
    use nondestructive::yaml::Document;

    mod nondestructive_yaml_bugs {
        use crate::app::yaml_edit::tests::new_doc;
        use crate::app::yaml_edit::{ValueExtensions, ValueMutExtensions};
        use assert2::{assert, let_assert};
        use indoc::indoc;
        use nondestructive::yaml::Separator;
        use test_r::test;

        // NOTE: if this breaks, that is means parsing is fixed (or at least changes) to handle comments at line ends,
        //       meaning we have to rework our workaround (search for workaround methods in this file)
        #[test]
        fn line_comments_are_part_of_values() {
            let doc = new_doc(indoc! {"
                map: # this works fine
                  key_for_string_with_comment: value # comment for string
                  key_for_string_without_comment: other values should not be affected by the workaround
                  key_for_number_with_comment: 3 # comment for number, this won't be parsed as number
                  key_for_number_without_comment: 4
                seq: # this is also okay
                - key_for_string_with_comment: value # comment for string
                  key_for_string_without_comment: other values should not be affected by the workaround
                  key_for_number_with_comment: 3 # comment for number, this won't be parsed as number
                  key_for_number_without_comment: 4
             "});

            {
                let map = doc
                    .as_ref()
                    .as_mapping()
                    .unwrap()
                    .get("map")
                    .unwrap()
                    .as_mapping()
                    .unwrap();
                let string_field_with_comment = map.get("key_for_string_with_comment").unwrap();
                let string_field_without_comment =
                    map.get("key_for_string_without_comment").unwrap();
                let number_with_comment_field = map.get("key_for_number_with_comment").unwrap();
                let number_without_comment_field =
                    map.get("key_for_number_without_comment").unwrap();
                assert!(string_field_with_comment.as_str() == Some("value # comment for string"));
                assert!(number_with_comment_field.as_number().is_none());
                assert!(
                    number_with_comment_field.as_str()
                        == Some("3 # comment for number, this won't be parsed as number")
                );
                assert!(number_without_comment_field.as_i64() == Some(4));

                // With workarounds
                assert!(
                    string_field_with_comment.as_str_with_comments_workaround() == Some("value")
                );
                assert!(
                    string_field_without_comment.as_str_with_comments_workaround()
                        == Some("other values should not be affected by the workaround")
                );
                assert!(number_with_comment_field.as_i64_with_comments_workaround() == Some(3));
            }

            {
                let seq = doc
                    .as_ref()
                    .as_mapping()
                    .unwrap()
                    .get("seq")
                    .unwrap()
                    .as_sequence()
                    .unwrap();
                let map = seq.get(0).unwrap().as_mapping().unwrap();
                let string_field_with_comment = map.get("key_for_string_with_comment").unwrap();
                let string_field_without_comment =
                    map.get("key_for_string_without_comment").unwrap();
                let number_with_comment_field = map.get("key_for_number_with_comment").unwrap();
                let number_without_comment_field =
                    map.get("key_for_number_without_comment").unwrap();
                assert!(string_field_with_comment.as_str() == Some("value # comment for string"));
                assert!(number_with_comment_field.as_number().is_none());
                assert!(
                    number_with_comment_field.as_str()
                        == Some("3 # comment for number, this won't be parsed as number")
                );
                assert!(number_without_comment_field.as_i64() == Some(4));

                // With workarounds
                assert!(
                    string_field_with_comment.as_str_with_comments_workaround() == Some("value")
                );
                assert!(
                    string_field_without_comment.as_str_with_comments_workaround()
                        == Some("other values should not be affected by the workaround")
                );
                assert!(number_with_comment_field.as_i64_with_comments_workaround() == Some(3));
            }
        }

        #[test]
        fn field_of_sequence_of_mapping_ident_bug() {
            // If there is only 1 mapping in a sequence that is part of a mapping, that results in
            // invalid YAML
            {
                let mut doc = new_doc("");
                let mut outer_map = doc.as_mut().into_mapping_replace_empty().unwrap();
                let mut seq = outer_map.insert("map", Separator::Auto).make_sequence();
                let mut map = seq.push(Separator::Auto).make_mapping();
                map.insert_str("first-key", "first-value");
                map.insert_str("second-key", "second-value");
                map.insert_str("third-key", "third-value");

                let doc_str = doc.to_string();
                println!("---\n{doc}");
                let_assert!(Err(error) = serde_yaml::from_str::<serde_yaml::Value>(&doc_str));
                println!("error: {error:#}");
            }

            // Workaround
            {
                let mut doc = new_doc("");
                let mut outer_map = doc.as_mut().into_mapping_replace_empty().unwrap();
                let mut seq = outer_map.insert("map", Separator::Auto).make_sequence();

                // Insert an empty map as first elem
                {
                    let _ = seq.push(Separator::Auto).make_mapping();
                }

                let mut map = seq.push(Separator::Auto).make_mapping();
                map.insert_str("first-key", "first-value");
                map.insert_str("second-key", "second-value");
                map.insert_str("third-key", "third-value");

                // Remove the empty map, this creates a "gap" as first elem, but also fixes the indent error
                {
                    seq.remove(0);
                }

                let doc_str = doc.to_string();
                println!("---\n{doc}");
                let serde_value = serde_yaml::from_str::<serde_yaml::Value>(&doc_str).unwrap();
                let seq = serde_value
                    .as_mapping()
                    .unwrap()
                    .get("map")
                    .unwrap()
                    .as_sequence()
                    .unwrap();
                assert!(seq.len() == 1);
                let map = seq[0].as_mapping().unwrap();
                assert!(map.get("first-key").unwrap() == "first-value");
                assert!(map.get("second-key").unwrap() == "second-value");
                assert!(map.get("third-key").unwrap() == "third-value");
            }
        }
    }

    mod into_mapping_replace_empty {
        use crate::app::yaml_edit::tests::{new_doc, to_serde_yaml_value};
        use crate::app::yaml_edit::ValueMutExtensions;
        use assert2::assert;
        use indoc::indoc;
        use test_r::test;

        #[test]
        fn into_mapping_replace_empty_with_really_empty() {
            let mut doc = new_doc("");

            let mut mapping = doc.as_mut().into_mapping_replace_empty().unwrap();
            mapping.insert_str("test:key", "");

            let serde_value = to_serde_yaml_value(&doc);
            assert!(serde_value.as_mapping().unwrap().get("test:key").is_some());
        }

        #[test]
        fn into_mapping_replace_empty_with_some_whitespace() {
            let mut doc = new_doc(indoc! {"


            "});

            let mut mapping = doc.as_mut().into_mapping_replace_empty().unwrap();
            mapping.insert_str("test:key", "");

            let serde_value = to_serde_yaml_value(&doc);
            assert!(serde_value.as_mapping().unwrap().get("test:key").is_some());
        }

        #[test]
        fn into_mapping_replace_empty_with_comments_and_whitespace() {
            let mut doc = new_doc(indoc! {"

                # I'm an empty document, with comments and whitespaces

                # and even more comments
                # and more

            "});

            let mut mapping = doc.as_mut().into_mapping_replace_empty().unwrap();
            mapping.insert_str("test:key", "");

            let serde_value = to_serde_yaml_value(&doc);
            assert!(serde_value.as_mapping().unwrap().get("test:key").is_some());
        }

        #[test]
        fn into_mapping_replace_empty_with_existing_mapping() {
            let mut doc = new_doc(indoc! {"
                another_key: with_value
            "});

            let mut mapping = doc.as_mut().into_mapping_replace_empty().unwrap();
            mapping.insert_str("test:key", "");

            let serde_value = to_serde_yaml_value(&doc);
            let serde_value = serde_value.as_mapping().unwrap();
            assert!(serde_value.get("test:key").is_some());
            assert!(serde_value.get("another_key").is_some());
            assert!(serde_value.get("another_key").unwrap().as_str() == Some("with_value"));
        }

        #[test]
        fn into_mapping_replace_empty_with_existing_non_empty_value() {
            let mut doc = new_doc(indoc! {"
                well
            "});

            assert!(doc.as_mut().into_mapping_replace_empty().is_err());
        }
    }

    mod into_sequence_replace_empty {
        use crate::app::yaml_edit::tests::{new_doc, to_serde_yaml_value};
        use crate::app::yaml_edit::ValueMutExtensions;
        use assert2::assert;
        use indoc::indoc;
        use test_r::test;

        #[test]
        fn into_sequence_replace_empty_with_really_empty() {
            let mut doc = new_doc("");

            let mut seq = doc.as_mut().into_sequence_replace_empty().unwrap();
            seq.push_string("test_elem");

            let serde_value = to_serde_yaml_value(&doc);
            assert!(serde_value
                .as_sequence()
                .unwrap()
                .iter()
                .any(|value| value.as_str() == Some("test_elem")));
        }

        #[test]
        fn into_sequence_replace_empty_with_some_whitespace() {
            let mut doc = new_doc(indoc! {"


            "});

            let mut seq = doc.as_mut().into_sequence_replace_empty().unwrap();
            seq.push_string("test_elem");

            let serde_value = to_serde_yaml_value(&doc);
            assert!(serde_value
                .as_sequence()
                .unwrap()
                .iter()
                .any(|value| value.as_str() == Some("test_elem")));
        }

        #[test]
        fn into_sequence_replace_empty_with_comments_and_whitespace() {
            let mut doc = new_doc(indoc! {"

                # I'm an empty document, with comments and whitespaces

                # and even more comments
                # and more

            "});

            let mut seq = doc.as_mut().into_sequence_replace_empty().unwrap();
            seq.push_string("test_elem");

            let serde_value = to_serde_yaml_value(&doc);
            assert!(serde_value
                .as_sequence()
                .unwrap()
                .iter()
                .any(|value| value.as_str() == Some("test_elem")));
        }

        #[test]
        fn into_sequence_replace_empty_with_existing_mapping() {
            let mut doc = new_doc(indoc! {"
                - another_elem
            "});

            let mut seq = doc.as_mut().into_sequence_replace_empty().unwrap();
            seq.push_string("test_elem");

            let serde_value = to_serde_yaml_value(&doc);
            let serde_value = serde_value.as_sequence().unwrap();
            assert!(serde_value
                .iter()
                .any(|value| value.as_str() == Some("test_elem")));
            assert!(serde_value
                .iter()
                .any(|value| value.as_str() == Some("another_elem")));
        }

        #[test]
        fn into_sequence_replace_empty_with_existing_non_empty_value() {
            let mut doc = new_doc(indoc! {"
                well
            "});

            assert!(doc.as_mut().into_sequence_replace_empty().is_err());
        }
    }

    fn new_doc(source: &str) -> Document {
        nondestructive::yaml::from_slice(source.as_bytes()).unwrap()
    }

    fn to_serde_yaml_value(doc: &Document) -> serde_yaml::Value {
        let doc_str = doc.to_string();
        println!("---\n{doc}\n");
        serde_yaml::from_str::<serde_yaml::Value>(&doc_str).unwrap()
    }
}
