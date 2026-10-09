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

pub(super) mod activity;
pub(super) mod metrics;
pub(super) mod resources;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum OpsDestination {
    Overview,
    Agents,
    Oplog,
    Stream,
    Metrics,
    Deployments,
    Configuration,
}

impl OpsDestination {
    pub(super) const ALL: [Self; 7] = [
        Self::Overview,
        Self::Agents,
        Self::Oplog,
        Self::Stream,
        Self::Metrics,
        Self::Deployments,
        Self::Configuration,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Agents => "Agents",
            Self::Oplog => "Oplog",
            Self::Stream => "Stream",
            Self::Metrics => "Metrics",
            Self::Deployments => "Deployments",
            Self::Configuration => "Configuration",
        }
    }

    pub(super) fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|destination| *destination == self)
            .unwrap_or_default()
    }

    pub(super) fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub(super) fn previous(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct AgentIdentity {
    pub(super) component: String,
    pub(super) agent_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct OplogRoute {
    pub(super) agents: Vec<AgentIdentity>,
    pub(super) focus: Option<OplogEntryIdentity>,
    pub(super) from_unix_nanos: Option<i64>,
    pub(super) to_unix_nanos: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct OplogEntryIdentity {
    pub(super) agent: AgentIdentity,
    pub(super) index: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum OpsRoute {
    Root,
    Agent(AgentIdentity),
    Oplog(OplogRoute),
    MetricsPanel(String),
}

#[derive(Debug)]
pub(super) struct OpsApp {
    active: OpsDestination,
    tab_offset: std::cell::Cell<usize>,
    routes: [Vec<OpsRoute>; OpsDestination::ALL.len()],
}

impl Default for OpsApp {
    fn default() -> Self {
        Self {
            active: OpsDestination::Overview,
            tab_offset: std::cell::Cell::new(0),
            routes: std::array::from_fn(|_| vec![OpsRoute::Root]),
        }
    }
}

impl OpsApp {
    pub(super) fn active(&self) -> OpsDestination {
        self.active
    }

    pub(super) fn select(&mut self, destination: OpsDestination) {
        self.active = destination;
    }

    pub(super) fn tab_offset(&self) -> usize {
        self.tab_offset.get()
    }

    pub(super) fn set_tab_offset(&self, offset: usize) {
        self.tab_offset
            .set(offset.min(OpsDestination::ALL.len().saturating_sub(1)));
    }

    pub(super) fn select_next(&mut self) -> OpsDestination {
        self.active = self.active.next();
        self.active
    }

    pub(super) fn select_previous(&mut self) -> OpsDestination {
        self.active = self.active.previous();
        self.active
    }

    pub(super) fn route(&self, destination: OpsDestination) -> &[OpsRoute] {
        &self.routes[destination.index()]
    }

    pub(super) fn current_route(&self) -> &OpsRoute {
        self.route(self.active).last().unwrap_or(&OpsRoute::Root)
    }

    pub(super) fn current_agent(&self) -> Option<&AgentIdentity> {
        match self.current_route() {
            OpsRoute::Agent(agent) => Some(agent),
            _ => None,
        }
    }

    pub(super) fn current_oplog(&self) -> Option<&OplogRoute> {
        match self.current_route() {
            OpsRoute::Oplog(route) => Some(route),
            _ => None,
        }
    }

    pub(super) fn clear_oplog_focus(&mut self) {
        if let Some(OpsRoute::Oplog(route)) = self.routes[OpsDestination::Oplog.index()].last_mut()
        {
            route.focus = None;
        }
    }

    pub(super) fn open_agent(&mut self, agent: AgentIdentity) {
        self.active = OpsDestination::Agents;
        self.routes[OpsDestination::Agents.index()].push(OpsRoute::Agent(agent));
    }

    pub(super) fn open_metrics_panel(&mut self, panel_id: String) {
        self.active = OpsDestination::Metrics;
        self.routes[OpsDestination::Metrics.index()].push(OpsRoute::MetricsPanel(panel_id));
    }

    pub(super) fn open_oplog(&mut self, route: OplogRoute) {
        self.active = OpsDestination::Oplog;
        self.routes[OpsDestination::Oplog.index()].push(OpsRoute::Oplog(route));
    }

    pub(super) fn pop_route(&mut self) -> bool {
        let route = &mut self.routes[self.active.index()];
        if route.len() <= 1 {
            return false;
        }
        route.pop();
        true
    }

    pub(super) fn reset_routes(&mut self) {
        for route in &mut self.routes {
            route.truncate(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    #[test]
    fn destination_cycle_is_stable_and_wraps() {
        assert_eq!(
            super::OpsDestination::Overview.previous(),
            super::OpsDestination::Configuration
        );
        assert_eq!(
            super::OpsDestination::Configuration.next(),
            super::OpsDestination::Overview
        );
    }

    #[test]
    fn routes_are_retained_per_destination_and_reset_together() {
        let mut app = super::OpsApp::default();
        app.open_agent(super::AgentIdentity {
            component: "component".to_string(),
            agent_id: "agent".to_string(),
        });
        app.select(super::OpsDestination::Metrics);
        assert_eq!(app.current_route(), &super::OpsRoute::Root);

        app.select(super::OpsDestination::Agents);
        assert!(matches!(app.current_route(), super::OpsRoute::Agent(_)));

        app.open_metrics_panel("invocation-rate".to_string());
        assert_eq!(
            app.current_route(),
            &super::OpsRoute::MetricsPanel("invocation-rate".to_string())
        );
        app.select(super::OpsDestination::Agents);
        assert!(matches!(app.current_route(), super::OpsRoute::Agent(_)));

        let agent = super::AgentIdentity {
            component: "component".to_string(),
            agent_id: "agent".to_string(),
        };
        app.open_oplog(super::OplogRoute {
            agents: vec![agent.clone()],
            focus: Some(super::OplogEntryIdentity { agent, index: 42 }),
            from_unix_nanos: None,
            to_unix_nanos: None,
        });
        assert!(matches!(app.current_route(), super::OpsRoute::Oplog(_)));
        app.select(super::OpsDestination::Metrics);
        assert_eq!(
            app.current_route(),
            &super::OpsRoute::MetricsPanel("invocation-rate".to_string())
        );
        app.reset_routes();
        assert_eq!(app.current_route(), &super::OpsRoute::Root);
    }
}
