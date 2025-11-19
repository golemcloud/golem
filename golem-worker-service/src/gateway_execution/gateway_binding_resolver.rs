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

use crate::gateway_router::{build_router, RouteEntry, RouterPattern};
use golem_service_base::custom_api::compiled_http_api_definition::CompiledHttpApiDefinition;

pub struct ResolvedRouteEntry {
    pub path_segments: Vec<String>,
    pub route_entry: RouteEntry,
}

pub async fn resolve_gateway_binding(
    compiled_api_definitions: Vec<CompiledHttpApiDefinition>,
    request: &poem::Request,
) -> Option<ResolvedRouteEntry> {
    let compiled_routes = compiled_api_definitions
        .iter()
        .flat_map(|x| {
            x.routes
                .iter()
                .map(|y| (x.account_id.clone(), x.environment_id.clone(), y.clone()))
        })
        .collect::<Vec<_>>();

    let router = build_router(compiled_routes);

    let path_segments: Vec<&str> = RouterPattern::split(request.uri().path()).collect();

    let route_entry = router.check_path(request.method(), &path_segments)?;

    Some(ResolvedRouteEntry {
        path_segments: path_segments.into_iter().map(|s| s.to_string()).collect(),
        route_entry: route_entry.clone(),
    })
}
