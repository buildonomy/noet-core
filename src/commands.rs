use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

use crate::{
    beliefset::Beliefs,
    config::NetworkRecord,
    properties::{AsRun, BeliefNode, Bid},
    query::{Focus, PaginatedQuery, ResultsPage},
};

/// Command interface between Tauri and the Buildonomy AppSessionContext global state
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    /// (Re)Load App config from tauri store.
    LoadNetworks,
    /// Replace the app config and associated BeliefNetwork.toml files to the commanded
    /// order and values
    SetNetworks(Vec<NetworkRecord>),
    /// Edit our local session context by adding a local directory and treating it as a belief network root
    GetNetFromDir,
    /// Get the text content and html content from a Bid
    GetProc(Bid, String),
    SetProc(String, String),
    /// Get saved focus map
    GetFocus,
    /// save focus map
    SetFocus(Focus),
    /// Return a BeliefSet corresponding to a focus object
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
            Op::GetNetFromDir => write!(f, "GetNetFromDir"),
            Op::GetProc(net, p) => write!(f, "GetProc({}:{})", net, p),
            Op::SetProc(p, _) => write!(f, "SetProc({})", p),
            Op::GetFocus => write!(f, "GetFocus"),
            Op::SetFocus(focus) => write!(
                f,
                "SetFocus(awareness: {}, radius: {}, attention: {})",
                focus.awareness.len(),
                focus.radius,
                focus
                    .attention
                    .iter()
                    .map(|ar| ar.doc_path.clone())
                    .collect::<Vec<String>>()
                    .join(", ")
            ),
            Op::GetStates(pq) => write!(f, "GetStates({:?})", pq),
        }
    }
}

/// Command interface between Tauri and the Buildonomy AppSessionContext global state
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpPayload {
    pub op: Op,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OpResult {
    Ok,
    Proc(AsRun),
    Focus(Focus),
    Page(ResultsPage<Beliefs>),
    Networks(Vec<NetworkRecord>),
    State(Beliefs),
    NetworkState(String, BeliefNode),
}

impl Display for OpResult {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            OpResult::Ok => write!(f, "Ok"),
            OpResult::Proc(p) => write!(f, "Proc({})", p.doc_path.clone()),
            OpResult::Focus(focus) => write!(
                f,
                "Focus(awareness: {}, radius: {}, attention: {})",
                focus.awareness.len(),
                focus.radius,
                focus
                    .attention
                    .iter()
                    .map(|ar| ar.doc_path.clone())
                    .collect::<Vec<String>>()
                    .join(", ")
            ),
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
