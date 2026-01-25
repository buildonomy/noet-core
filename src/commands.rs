use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

use crate::{
    beliefbase::BeliefGraph,
    config::NetworkRecord,
    properties::BeliefNode,
    query::{PaginatedQuery, ResultsPage},
};

/// Command interface for noet-core library operations
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    /// (Re)Load network configuration
    LoadNetworks,
    /// Replace the network configuration with the commanded order and values
    SetNetworks(Vec<NetworkRecord>),
    /// Update content at a specific path
    UpdateContent(String, String),
    /// Return a BeliefBase corresponding to a paginated query
    GetStates(PaginatedQuery),
}

impl Display for Op {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            Op::LoadNetworks => write!(f, "LoadNetworks"),
            Op::SetNetworks(v) => write!(
                f,
                "SetNetworks({})",
                v.iter()
                    .map(|r| r.path.clone())
                    .collect::<Vec<String>>()
                    .join(", ")
            ),
            Op::UpdateContent(p, _) => write!(f, "UpdateContent({p})"),
            Op::GetStates(pq) => write!(f, "GetStates({pq:?})"),
        }
    }
}

/// Command payload wrapper
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpPayload {
    pub op: Op,
}

/// Result of executing a command operation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OpResult {
    Ok,
    Page(ResultsPage<BeliefGraph>),
    Networks(Vec<NetworkRecord>),
    State(BeliefGraph),
    NetworkState(String, BeliefNode),
}

impl Display for OpResult {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            OpResult::Ok => write!(f, "Ok"),
            OpResult::Page(r) => write!(
                f,
                "Page({}-{} of {} items)",
                r.start,
                r.start + r.results.states.len(),
                r.count
            ),
            OpResult::Networks(v) => write!(
                f,
                "Networks({})",
                v.iter()
                    .map(|r| r.path.clone())
                    .collect::<Vec<String>>()
                    .join(", ")
            ),
            OpResult::State(_) => write!(f, "State"),
            OpResult::NetworkState(_, _) => write!(f, "NetworkState"),
        }
    }
}
