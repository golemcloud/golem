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

use golem_service_base::custom_api::{
    ConstructorParameter, MethodParameter, PathSegment, PathSegmentType, QueryOrHeaderType,
};
use golem_service_base::model::SafeIndex;

pub fn get_query_variable_and_types(
    method_params: &[MethodParameter],
) -> Vec<(&str, &QueryOrHeaderType)> {
    method_params
        .iter()
        .filter_map(|p| {
            if let MethodParameter::Query {
                query_parameter_name,
                parameter_type,
            } = p
            {
                Some((query_parameter_name.as_str(), parameter_type))
            } else {
                None
            }
        })
        .collect()
}

pub fn get_header_variable_and_types(
    method_params: &[MethodParameter],
) -> Vec<(&str, &QueryOrHeaderType)> {
    method_params
        .iter()
        .filter_map(|p| {
            if let MethodParameter::Header {
                header_name,
                parameter_type,
            } = p
            {
                Some((header_name.as_str(), parameter_type))
            } else {
                None
            }
        })
        .collect()
}

pub fn get_path_variables_and_types<'a>(
    path_segments: &'a [PathSegment],
    constructor_parameter: &'a [ConstructorParameter],
    method_params: &'a [MethodParameter],
) -> Vec<(&'a str, bool, &'a PathSegmentType)> {
    let input_path_variable_types =
        collect_path_variable_types(constructor_parameter, method_params);
    build_path_segments_and_types(path_segments, input_path_variable_types)
}

fn collect_path_variable_types<'a>(
    constructor_params: &'a [ConstructorParameter],
    method_params: &'a [MethodParameter],
) -> Vec<(SafeIndex, &'a PathSegmentType)> {
    let mut types = Vec::new();

    let constructor_types = constructor_params
        .iter()
        .map(|p| match p {
            ConstructorParameter::Path {
                parameter_type,
                path_segment_index,
            } => (*path_segment_index, parameter_type),
        })
        .collect::<Vec<_>>();

    let method_types = method_params
        .iter()
        .filter_map(|p| match p {
            MethodParameter::Path {
                parameter_type,
                path_segment_index,
            } => Some((*path_segment_index, parameter_type)),
            _ => None,
        })
        .collect::<Vec<_>>();

    types.extend(constructor_types);
    types.extend(method_types);
    types
}

fn build_path_segments_and_types<'a>(
    path_segments: &'a [PathSegment],
    segment_types: Vec<(SafeIndex, &'a PathSegmentType)>,
) -> Vec<(&'a str, bool, &'a PathSegmentType)> {
    let mut path_params_and_types = Vec::new();
    let mut path_variable_index = SafeIndex::new(0);

    for segment in path_segments.iter() {
        match segment {
            PathSegment::Literal { .. } => {}

            PathSegment::Variable { display_name } => {
                let (_, var_type) = segment_types
                    .iter()
                    .find(|(index, _)| *index == path_variable_index)
                    .expect("Failed to find path variable index in agent parameters");

                path_params_and_types.push((display_name.as_str(), false, *var_type));
                path_variable_index += 1;
            }

            PathSegment::CatchAll { display_name } => {
                let (_, var_type) = segment_types
                    .iter()
                    .find(|(index, _)| *index == path_variable_index)
                    .expect("Failed to find path variable index in agent parameters");

                path_params_and_types.push((display_name.as_str(), true, *var_type));
                path_variable_index += 1;
            }
        }
    }
    path_params_and_types
}
