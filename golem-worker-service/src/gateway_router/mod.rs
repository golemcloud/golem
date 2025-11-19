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

mod core;
mod pattern;
pub mod tree;

pub use core::*;
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::worker_api::compiled_gateway_binding::GatewayBindingCompiled;
use golem_service_base::worker_api::compiled_http_api_definition::CompiledRoute;
use golem_service_base::worker_api::http_middlewares::HttpMiddlewares;
use golem_service_base::worker_api::path_pattern::{PathPattern, QueryInfo, VarInfo};
pub use pattern::*;

#[derive(Debug, Clone)]
pub enum PathParamExtractor {
    Single { var_info: VarInfo, index: usize },
    AllFollowing { var_info: VarInfo, index: usize },
}

#[derive(Debug, Clone)]
pub struct RouteEntry {
    // size is the index of all path patterns.
    pub path_params: Vec<PathParamExtractor>,
    pub query_params: Vec<QueryInfo>,
    pub binding: GatewayBindingCompiled,
    pub middlewares: Option<HttpMiddlewares>,
    pub account_id: AccountId,
    pub environment_id: EnvironmentId,
}

pub fn build_router(routes: Vec<(AccountId, EnvironmentId, CompiledRoute)>) -> Router<RouteEntry> {
    let mut router = Router::new();

    for (account_id, environment_id, route) in routes {
        let method = route.method.into();
        let path = route.path;
        let binding = route.binding;

        let path_params = path
            .path_patterns
            .iter()
            .enumerate()
            .filter_map(|(i, x)| match x {
                PathPattern::Var(var_info) => Some(PathParamExtractor::Single {
                    var_info: var_info.clone(),
                    index: i,
                }),
                PathPattern::CatchAllVar(var_info) => Some(PathParamExtractor::AllFollowing {
                    var_info: var_info.clone(),
                    index: i,
                }),
                _ => None,
            })
            .collect();

        let entry = RouteEntry {
            path_params,
            query_params: path.query_params,
            binding,
            middlewares: route.middlewares,
            account_id,
            environment_id,
        };

        let path: Vec<RouterPattern> = path
            .path_patterns
            .iter()
            .map(|x| x.clone().into())
            .collect();

        router.add_route(method, path, entry);
    }

    router
}
