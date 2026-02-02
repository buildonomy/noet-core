use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::path::PathBuf;

use crate::{
    nodekey::NodeKey,
    properties::{Bid, Weight, WeightKind, WeightSet},
};

/// Indicates the origin of a BeliefEvent for proper handling by different cache implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EventOrigin {
    /// Event was generated locally by this BeliefBase and has already been applied to its state.
    /// BeliefBase should validate consistency but skip reapplication.
    /// Other caches (DbConnection, remote syncs) should apply these events.
    Local,

    /// Event came from an external source (DbConnection restore, file watcher, network sync).
    /// BeliefBase must apply these events to synchronize state.
    #[default]
    Remote,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BeliefEvent {
    /// Keys mapping to old node, toml-serialized node
    NodeUpdate(Vec<NodeKey>, String, EventOrigin),
    NodesRemoved(Vec<Bid>, EventOrigin),
    /// From ID, To ID
    NodeRenamed(Bid, Bid, EventOrigin),
    /// Network updated, list of new (path, node, order) tuples
    PathAdded(Bid, String, Bid, Vec<u16>, EventOrigin),
    PathUpdate(Bid, String, Bid, Vec<u16>, EventOrigin),
    PathsRemoved(Bid, Vec<String>, EventOrigin),
    /// Source, Sink, WeightSet, EventOrigin)
    RelationUpdate(Bid, Bid, WeightSet, EventOrigin),
    /// Source, Sink, WeightKind, weight_payload, event origin
    RelationChange(Bid, Bid, WeightKind, Option<Weight>, EventOrigin),
    /// Source, Sink relation removed
    RelationRemoved(Bid, Bid, EventOrigin),
    /// File successfully parsed - track mtime for cache invalidation
    FileParsed(PathBuf),
    /// A signal that the BeliefBase should be balanced at this point.
    BalanceCheck,
    /// A signal to run a full built in test.
    BuiltInTest,
}

impl PartialEq for BeliefEvent {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::NodeUpdate(l0, l1, l2), Self::NodeUpdate(r0, r1, r2)) => {
                l0 == r0 && l1 == r1 && l2 == r2
            }
            (Self::NodesRemoved(l0, l1), Self::NodesRemoved(r0, r1)) => l0 == r0 && l1 == r1,
            (Self::NodeRenamed(l0, l1, l2), Self::NodeRenamed(r0, r1, r2)) => {
                l0 == r0 && l1 == r1 && l2 == r2
            }
            (Self::PathAdded(l0, l1, l2, l3, l4), Self::PathAdded(r0, r1, r2, r3, r4)) => {
                l0 == r0 && l1 == r1 && l2 == r2 && l3 == r3 && l4 == r4
            }
            (Self::PathUpdate(l0, l1, l2, l3, l4), Self::PathUpdate(r0, r1, r2, r3, r4)) => {
                l0 == r0 && l1 == r1 && l2 == r2 && l3 == r3 && l4 == r4
            }
            (Self::PathsRemoved(l0, l1, l2), Self::PathsRemoved(r0, r1, r2)) => {
                l0 == r0 && l1 == r1 && l2 == r2
            }
            (Self::RelationUpdate(l0, l1, l2, l3), Self::RelationUpdate(r0, r1, r2, r3)) => {
                l0 == r0 && l1 == r1 && l2 == r2 && l3 == r3
            }
            (
                Self::RelationChange(l0, l1, l2, l3, l4),
                Self::RelationChange(r0, r1, r2, r3, r4),
            ) => l0 == r0 && l1 == r1 && l2 == r2 && l3 == r3 && l4 == r4,
            (Self::RelationRemoved(l0, l1, l2), Self::RelationRemoved(r0, r1, r2)) => {
                l0 == r0 && l1 == r1 && l2 == r2
            }
            (Self::FileParsed(l0), Self::FileParsed(r0)) => l0 == r0,
            _ => false,
        }
    }
}

impl Eq for BeliefEvent {}

impl BeliefEvent {
    /// Returns the EventOrigin of this event, or None for BalanceCheck
    pub fn origin(&self) -> Option<EventOrigin> {
        match self {
            BeliefEvent::NodeUpdate(_, _, origin) => Some(*origin),
            BeliefEvent::NodesRemoved(_, origin) => Some(*origin),
            BeliefEvent::NodeRenamed(_, _, origin) => Some(*origin),
            BeliefEvent::PathAdded(_, _, _, _, origin) => Some(*origin),
            BeliefEvent::PathUpdate(_, _, _, _, origin) => Some(*origin),
            BeliefEvent::PathsRemoved(_, _, origin) => Some(*origin),
            BeliefEvent::RelationUpdate(_, _, _, origin) => Some(*origin),
            BeliefEvent::RelationChange(_, _, _, _, origin) => Some(*origin),
            BeliefEvent::RelationRemoved(_, _, origin) => Some(*origin),
            BeliefEvent::FileParsed(_) => None,
            BeliefEvent::BalanceCheck => None,
            BeliefEvent::BuiltInTest => None,
        }
    }

    /// Returns a new event with the specified origin
    pub fn with_origin(self, new_origin: EventOrigin) -> Self {
        match self {
            BeliefEvent::NodeUpdate(k, s, _) => BeliefEvent::NodeUpdate(k, s, new_origin),
            BeliefEvent::NodesRemoved(b, _) => BeliefEvent::NodesRemoved(b, new_origin),
            BeliefEvent::NodeRenamed(f, t, _) => BeliefEvent::NodeRenamed(f, t, new_origin),
            BeliefEvent::PathAdded(n, p, b, o, _) => BeliefEvent::PathAdded(n, p, b, o, new_origin),
            BeliefEvent::PathUpdate(n, p, b, o, _) => {
                BeliefEvent::PathUpdate(n, p, b, o, new_origin)
            }
            BeliefEvent::PathsRemoved(n, p, _) => BeliefEvent::PathsRemoved(n, p, new_origin),
            BeliefEvent::RelationUpdate(s, k, w, _) => {
                BeliefEvent::RelationUpdate(s, k, w, new_origin)
            }
            BeliefEvent::RelationChange(s, k, wk, p, _) => {
                BeliefEvent::RelationChange(s, k, wk, p, new_origin)
            }
            BeliefEvent::RelationRemoved(s, k, _) => BeliefEvent::RelationRemoved(s, k, new_origin),
            BeliefEvent::FileParsed(p) => BeliefEvent::FileParsed(p),
            BeliefEvent::BalanceCheck => BeliefEvent::BalanceCheck,
            BeliefEvent::BuiltInTest => BeliefEvent::BuiltInTest,
        }
    }
}

impl Display for BeliefEvent {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            BeliefEvent::NodeUpdate(_, _, _) => write!(f, "NodeUpdate"),
            BeliefEvent::NodesRemoved(_, _) => write!(f, "NodesRemoved"),
            BeliefEvent::NodeRenamed(_, _, _) => write!(f, "NodeRenamed"),
            BeliefEvent::PathAdded(_, _, _, _, _) => write!(f, "PathAdded"),
            BeliefEvent::PathUpdate(_, _, _, _, _) => write!(f, "PathUpdate"),
            BeliefEvent::PathsRemoved(_, _, _) => write!(f, "PathsRemoved"),
            BeliefEvent::RelationUpdate(_, _, _, _) => write!(f, "RelationUpdate"),
            BeliefEvent::RelationChange(_, _, _, _, _) => write!(f, "RelationChange"),
            BeliefEvent::RelationRemoved(_, _, _) => write!(f, "RelationRemoved"),
            BeliefEvent::FileParsed(_) => write!(f, "FileParsed"),
            BeliefEvent::BalanceCheck => write!(f, "BalanceCheck"),
            BeliefEvent::BuiltInTest => write!(f, "BuiltInTest"),
        }
    }
}

// // TODO: Make perceptionevent into a trait? Such that different value types can be tracked by the compiler
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PerceptionEvent {
    /// An event to automatically fill out an input field within a procedure
    Input(Bid, String),
    /// A participant focus event (hover/select within a table of contents or belief link for example)
    Focus(Bid),
    /// A change of awareness that should result in a change to the BeliefBase store within the UI
    Awareness(Vec<Bid>),
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    #[default]
    Ping,
    Belief(BeliefEvent),
    // Perception(PerceptionEvent),
    Focus(PerceptionEvent),
}
