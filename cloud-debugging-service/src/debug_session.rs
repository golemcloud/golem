use async_trait::async_trait;
use cloud_common::auth::CloudNamespace;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{OwnedWorkerId, WorkerId};
use serde::Serialize;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::{Arc, Mutex, RwLock};

// A shared debug session which will be internally used by the custom oplog service
// dedicated to running debug executor
#[async_trait]
pub trait DebugSession {
    async fn insert(
        &self,
        debug_session_id: DebugSessionId,
        session_value: DebugSessionData,
    ) -> DebugSessionId;
    async fn get(&self, debug_session_id: &DebugSessionId) -> Option<DebugSessionData>;

    async fn remove(&self, debug_session_id: DebugSessionId) -> Option<DebugSessionData>;
}
pub struct DebugSessionDefault {
    pub session: Arc<Mutex<HashMap<DebugSessionId, DebugSessionData>>>,
}

impl DebugSessionDefault {
    pub fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl DebugSession for DebugSessionDefault {
    async fn insert(
        &self,
        debug_session_id: DebugSessionId,
        session_value: DebugSessionData,
    ) -> DebugSessionId {
        let mut session = self.session.lock().unwrap();
        session.insert(debug_session_id.clone(), session_value);
        debug_session_id
    }

    async fn get(&self, debug_session_id: &DebugSessionId) -> Option<DebugSessionData> {
        let session = self.session.lock().unwrap();
        session.get(debug_session_id).cloned()
    }

    async fn remove(&self, debug_session_id: DebugSessionId) -> Option<DebugSessionData> {
        let mut session = self.session.lock().unwrap();
        session.remove(&debug_session_id)
    }
}

#[derive(Clone)]
pub struct DebugSessionData {
    pub target_oplog_index: Option<OplogIndex>, // Add more info here
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DebugSessionId(OwnedWorkerId);

impl Serialize for DebugSessionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.to_string().serialize(serializer)
    }
}

impl DebugSessionId {
    pub fn new(worker_id: OwnedWorkerId) -> Self {
        DebugSessionId(worker_id)
    }

    pub fn worker_id(&self) -> WorkerId {
        self.0.worker_id()
    }
}
impl Display for DebugSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone)]
pub struct ActiveSessionData {
    pub cloud_namespace: CloudNamespace,
    pub worker_id: WorkerId,
}

impl ActiveSessionData {
    pub fn new(cloud_namespace: CloudNamespace, worker_id: WorkerId) -> Self {
        Self {
            cloud_namespace,
            worker_id,
        }
    }
}

#[derive(Default)]
pub struct ActiveSession {
    pub active_session: Arc<RwLock<Option<ActiveSessionData>>>,
}

impl ActiveSession {
    pub async fn set_active_session(&self, worker_id: WorkerId, cloud_namespace: CloudNamespace) {
        let mut active_session = self.active_session.write().unwrap();
        *active_session = Some(ActiveSessionData::new(cloud_namespace, worker_id));
    }

    pub async fn get_active_session(&self) -> Option<ActiveSessionData> {
        let active_session = &self.active_session.read().unwrap();
        let active_session = active_session.as_ref();
        active_session.cloned()
    }
}
