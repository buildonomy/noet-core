use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

use crate::{beliefbase::BeliefGraph, config::NetworkRecord, properties::BeliefNode};

/// Command interface for noet-core library operations
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    /// (Re)Load network configuration
    LoadNetworks,
    /// Replace the network configuration with the commanded order and values
    SetNetworks(Vec<NetworkRecord>),
    /// Update content at a specific path
    UpdateContent(String, String),
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
    Networks(Vec<NetworkRecord>),
    State(BeliefGraph),
    NetworkState(String, BeliefNode),
}

impl Display for OpResult {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            OpResult::Ok => write!(f, "Ok"),
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
