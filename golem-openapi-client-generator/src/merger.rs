use indexmap::IndexMap;
use openapiv3::{Components, ExternalDocumentation, OpenAPI, Paths};

use crate::Error;
use crate::Result;

pub fn merge_all_openapi_specs(openapi_specs: Vec<OpenAPI>) -> Result<OpenAPI> {
    if openapi_specs.is_empty() {
        Err(Error::unexpected("No OpenAPI specs provided"))
    } else if openapi_specs.len() == 1 {
        Ok(openapi_specs.into_iter().next().unwrap())
    } else {
        let mut openapi_specs = openapi_specs;
        let first = openapi_specs.pop().unwrap();
        let rest = openapi_specs;
        rest.into_iter().try_fold(first, merge_openapi_specs)
    }
}

fn merge_openapi_specs(a: OpenAPI, b: OpenAPI) -> Result<OpenAPI> {
    let openapi_version = {
        if a.openapi != b.openapi {
            return Err(Error::unexpected("OpenAPI versions do not match"));
        }
        a.openapi
    };

    let info = {
        if a.info != b.info {
            return Err(Error::unexpected("Info objects do not match"));
        }
        a.info
    };

    let servers = {
        if a.servers != b.servers {
            return Err(Error::unexpected("Servers do not match"));
        }
        a.servers
    };

    let all_tags = {
        let a_tags_map = a
            .tags
            .into_iter()
            .map(|tag| (tag.name.clone(), tag))
            .collect::<IndexMap<_, _>>();
        let b_tags_map = b
            .tags
            .into_iter()
            .map(|tag| (tag.name.clone(), tag))
            .collect::<IndexMap<_, _>>();
        let merged = merge_unique(a_tags_map, b_tags_map)?;

        merged.into_values().collect::<Vec<_>>()
    };

    let all_paths = {
        let Paths {
            paths: a_paths,
            extensions: a_extensions,
        } = a.paths;
        let Paths {
            paths: b_paths,
            extensions: b_extensions,
        } = b.paths;
        let all_paths = merge_unique(a_paths, b_paths)?;
        let all_extensions = merge_unique(a_extensions, b_extensions)?;
        Paths {
            paths: all_paths,
            extensions: all_extensions,
        }
    };

    let components = merge_components(a.components, b.components)?;
    let security = merge_unique_option_list(a.security, b.security);
    let extensions = merge_unique(a.extensions, b.extensions)?;

    let external_docs = merge_external_docs(a.external_docs, b.external_docs)?;

    let result = OpenAPI {
        openapi: openapi_version,
        info,
        servers,
        paths: all_paths,
        components,
        security,
        tags: all_tags,
        extensions,
        external_docs,
    };

    Ok(result)
}

fn merge_components(a: Option<Components>, b: Option<Components>) -> Result<Option<Components>> {
    let result = match (a, b) {
        (Some(a), Some(b)) => {
            let Components {
                schemas: a_schemas,
                responses: a_responses,
                parameters: a_parameters,
                examples: a_examples,
                request_bodies: a_request_bodies,
                headers: a_headers,
                security_schemes: a_security_schemes,
                links: a_links,
                callbacks: a_callbacks,
                extensions: a_extensions,
            } = a;

            let Components {
                schemas: b_schemas,
                responses: b_responses,
                parameters: b_parameters,
                examples: b_examples,
                request_bodies: b_request_bodies,
                headers: b_headers,
                security_schemes: b_security_schemes,
                links: b_links,
                callbacks: b_callbacks,
                extensions: b_extensions,
            } = b;

            let merged = Components {
                schemas: merge_unique(a_schemas, b_schemas)?,
                responses: merge_unique(a_responses, b_responses)?,
                parameters: merge_unique(a_parameters, b_parameters)?,
                examples: merge_unique(a_examples, b_examples)?,
                request_bodies: merge_unique(a_request_bodies, b_request_bodies)?,
                headers: merge_unique(a_headers, b_headers)?,
                security_schemes: merge_unique(a_security_schemes, b_security_schemes)?,
                links: merge_unique(a_links, b_links)?,
                callbacks: merge_unique(a_callbacks, b_callbacks)?,
                extensions: merge_unique(a_extensions, b_extensions)?,
            };
            Some(merged)
        }
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    Ok(result)
}

fn merge_external_docs(
    a: Option<ExternalDocumentation>,
    b: Option<ExternalDocumentation>,
) -> crate::Result<Option<ExternalDocumentation>> {
    let result = match (a, b) {
        (Some(a), Some(b)) => {
            let ExternalDocumentation {
                description: a_description,
                url: a_url,
                extensions: a_extensions,
            } = a;

            let ExternalDocumentation {
                description: b_description,
                url: b_url,
                extensions: b_extensions,
            } = b;

            let description = match (a_description, b_description) {
                (Some(a), Some(b)) => {
                    if a != b {
                        return Err(Error::unexpected(
                            "External documentation descriptions do not match",
                        ));
                    }
                    Some(a)
                }
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };

            let url = {
                if a_url != b_url {
                    return Err(Error::unexpected(
                        "External documentation URLs do not match",
                    ));
                }
                a_url
            };

            let extensions = merge_unique(a_extensions, b_extensions)?;

            Some(ExternalDocumentation {
                description,
                url,
                extensions,
            })
        }
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    Ok(result)
}

fn merge_unique_option_list<Key, Item>(
    a: Option<Vec<IndexMap<Key, Item>>>,
    b: Option<Vec<IndexMap<Key, Item>>>,
) -> Option<Vec<IndexMap<Key, Item>>> {
    match (a, b) {
        (Some(a), Some(mut b)) => {
            let mut result = a;
            result.append(&mut b);
            Some(result)
        }
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn merge_unique<Key, Item>(
    mut a: IndexMap<Key, Item>,
    b: IndexMap<Key, Item>,
) -> Result<IndexMap<Key, Item>>
where
    Key: std::fmt::Debug + Eq + std::hash::Hash,
    Item: std::fmt::Debug + PartialEq,
{
    for (key, value) in b {
        match a.entry(key) {
            indexmap::map::Entry::Occupied(entry) => {
                if entry.get() != &value {
                    return Err(Error::unexpected(format!(
                        "Duplicate key {:?} with different values \n Current {:?} \n New {:?}",
                        entry.key(),
                        entry.get(),
                        value
                    )));
                }
            }
            indexmap::map::Entry::Vacant(entry) => {
                entry.insert(value);
            }
        }
    }
    Ok(a)
}
