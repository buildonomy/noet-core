pub use enumset::EnumSet;
/// [crate::properties] contains the basic building blocks for assembling and manipulating
/// [crate::beliefbase::BeliefBase]s and associated structures.
use enumset::*;
use petgraph::IntoWeightedEdge;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    cmp::Ordering,
    collections::BTreeSet,
    fmt::{self, Display, Formatter},
    hash::{Hash, Hasher},
    mem::replace,
    ops::{Deref, DerefMut},
};
use toml::{from_str, to_string, value::Table, Value};

pub use uuid::Uuid;
// Use `Uuid` as a custom type, with `String` as the Builtin
uniffi::custom_type!(Uuid, String, {
    remote,
    try_lift: |val| Ok(Uuid::try_from(val)?),
    lower: |obj| format!(
        "{}",
        obj.hyphenated().encode_lower(&mut Uuid::encode_buffer())
    )
});

uniffi::custom_type!(Table, String, {
    remote,
    try_lift: |val: String| -> Result<Table, BuildonomyError> {
        Ok(toml::from_str(&val)?)
    },
    lower: |obj: Table| -> String {
        toml::to_string(&obj).unwrap_or_default()
    },
});

#[cfg(feature = "service")]
use sqlx::{sqlite::SqliteRow, FromRow, Row};

use crate::{
    beliefbase::BeliefBase,
    error::BuildonomyError,
    nodekey::NodeKey,
    paths::{as_anchor, to_anchor, AnchorPath},
};

#[cfg(not(target_arch = "wasm32"))]
use crate::codec::belief_ir::ProtoBeliefNode;

pub(crate) mod enumset_list {
    // Copied from enumset_derive/src/lib.rs SerdeRepr::List (line 475 in version 0.10.1)
    use crate::properties::{BeliefKind, BeliefKindSet};
    use enumset::EnumSet;
    use serde::{ser::SerializeSeq, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(set: &BeliefKindSet, ser: S) -> Result<S::Ok, S::Error> {
        use SerializeSeq;
        let mut seq = ser.serialize_seq(Some(set.0.len()))?;
        for bit in set.0.iter() {
            seq.serialize_element(&bit)?;
        }
        seq.end()
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        de: D,
    ) -> core::result::Result<BeliefKindSet, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = BeliefKindSet;
            fn expecting(&self, formatter: &mut core::fmt::Formatter) -> core::fmt::Result {
                write!(formatter, "A list of BeliefKind values")
            }
            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut accum = EnumSet::<BeliefKind>::new();
                while let Some(val) = seq.next_element::<BeliefKind>()? {
                    accum |= val;
                }
                Ok(BeliefKindSet(accum))
            }
        }
        de.deserialize_seq(Visitor)
    }
}

/// The Buildonomy namespace UUID. This is used to create an anchor node within
/// [`crate::beliefbase::BidGraph`]`s in order to identify the top of the graph, as well as identify what
/// buildonomy core API the beliefbase structure complies to.
pub const UUID_NAMESPACE_BUILDONOMY: Uuid = Uuid::from_bytes([
    0x6b, 0x3d, 0x21, 0x54, 0xc0, 0xa9, 0x43, 0x7b, 0x93, 0x24, 0x5f, 0x62, 0xad, 0xeb, 0x9a, 0x44,
]);

/// The 'href' namespace UUID. This is used to create a universal network location for tracking
/// external facing http/https links within source documents.
pub const UUID_NAMESPACE_HREF: Uuid = Uuid::from_bytes([
    0x5b, 0x3d, 0x21, 0x54, 0xc0, 0xa9, 0x43, 0x7b, 0x93, 0x24, 0x5f, 0x62, 0xad, 0xeb, 0x9a, 0x44,
]);

/// The 'asset' namespace UUID. This is used to create a universal network location for tracking
/// assets (images, CSS, fonts, etc.) referenced within source documents.
pub const UUID_NAMESPACE_ASSET: Uuid = Uuid::from_bytes([
    0x4b, 0x3d, 0x21, 0x54, 0xc0, 0xa9, 0x43, 0x7b, 0x93, 0x24, 0x5f, 0x62, 0xad, 0xeb, 0x9a, 0x44,
]);

#[uniffi::export]
pub fn buildonomy_namespace() -> Bid {
    Bid::from(UUID_NAMESPACE_BUILDONOMY)
}

#[uniffi::export]
pub fn href_namespace() -> Bid {
    Bid::from(UUID_NAMESPACE_HREF)
}

#[uniffi::export]
pub fn asset_namespace() -> Bid {
    Bid::from(UUID_NAMESPACE_ASSET)
}

/// Generate a versioned API BID within the Buildonomy namespace
///
/// This creates a deterministic BID by:
/// 1. Generating a UUID v5 from the version string
/// 2. Replacing octets 10-15 with BUILDONOMY_NAMESPACE_BYTES
///
/// This approach:
/// - Makes each version's BID deterministic (same version = same BID)
/// - Keeps the namespace bytes in the standard location (octets 10-15)
/// - Allows is_reserved() to detect all API BIDs by checking those bytes
///
/// # Example
/// ```
/// # use noet_core::properties::buildonomy_api_bid;
/// let api_v0 = buildonomy_api_bid("0.0.0");
/// assert!(api_v0.is_reserved());
/// ```
pub fn buildonomy_api_bid(version: &str) -> Bid {
    // Generate a UUID v5 for deterministic versioning
    let uuid = Uuid::new_v5(&UUID_NAMESPACE_BUILDONOMY, version.as_bytes());

    // Replace octets 10-15 with Buildonomy namespace bytes
    // This makes the BID detectable as reserved while keeping it deterministic
    let mut bytes = *uuid.as_bytes();
    bytes[10..16].copy_from_slice(buildonomy_namespace().bref().bytes());

    Bid(Uuid::from_bytes(bytes))
}

pub fn buildonomy_asset_bid(hash_str: &str) -> Bid {
    // Generate a UUID v5 for deterministic versioning
    let uuid = Uuid::new_v5(&UUID_NAMESPACE_ASSET, hash_str.as_bytes());

    // Replace octets 10-15 with Buildonomy namespace bytes
    // This makes the BID detectable as reserved while keeping it deterministic
    let mut bytes = *uuid.as_bytes();
    bytes[10..16].copy_from_slice(asset_namespace().bref().bytes());

    Bid(Uuid::from_bytes(bytes))
}

pub fn buildonomy_href_bid(hash_str: &str) -> Bid {
    // Generate a UUID v5 for deterministic versioning
    let uuid = Uuid::new_v5(&UUID_NAMESPACE_HREF, hash_str.as_bytes());

    // Replace octets 10-15 with Buildonomy namespace bytes
    // This makes the BID detectable as reserved while keeping it deterministic
    let mut bytes = *uuid.as_bytes();
    bytes[10..16].copy_from_slice(href_namespace().bref().bytes());

    Bid(Uuid::from_bytes(bytes))
}

pub const BID_NAMESPACE_NIL: [u8; 6] = [0; 6];

/// Create a [Uuid::new_v5] using an input UUID mixed with the [UUID_NAMESPACE_BUILDONOMY]. The
/// least significant 48bits (octets 10-15) are used by Belief IDs to associate `BeliefNode`s within
/// their source context. See [crate::properties::Bid].
pub fn generate_namespace<U: AsRef<Uuid>>(node: U) -> Bid {
    Bid(Uuid::new_v5(
        &UUID_NAMESPACE_BUILDONOMY,
        node.as_ref().as_bytes(),
    ))
}

/// Belief ID
///
/// A UUID (v7) where the node ID is generated from a predecessor ID by generating a UUID v5 from
/// the prececessor combined with the [UUID_NAMESPACE_BUILDONOMY] UUID. In this
/// manner, embedded and derived symbols can be natively expressed intrinsically by the assigned
/// universal IDs.
///
/// Because Bid's are v6 Uuids, they are Ord, and arranged first chronologically by system time
/// within the generating process, then by node namespace.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Bid(Uuid);

uniffi::custom_newtype!(Bid, Uuid);

impl Bid {
    pub fn new<U: AsRef<Bid>>(parent: U) -> Self {
        Bid(Uuid::now_v6(parent.as_ref().bref().bytes()))
    }

    /// Use a [Bid::nil] when generating temporary ids in order to identify that the item has no
    /// known source context.
    pub fn nil() -> Self {
        Bid(Uuid::nil())
    }

    pub fn is_nil(&self) -> bool {
        *self == Bid::nil()
    }

    pub fn initialized(&self) -> bool {
        *self.parent_bref().bytes() != BID_NAMESPACE_NIL
    }

    /// Mutates the BID's namespace to match the parent namespace ID. This is useful for
    /// transforming uninitialized BIDs (generated from [Bid::default] or [Bid::nil]) into
    /// initialized BIDs.
    pub fn adopt_into(&mut self, parent: &Bid) -> Bid {
        let mut self_bytes = *self.0.as_bytes();
        self_bytes[10..16].copy_from_slice(parent.bref().bytes());
        let _ = replace(&mut self.0, Uuid::from_bytes(self_bytes));
        *self
    }

    /// Check if this BID falls within the reserved Buildonomy API namespace
    ///
    /// Returns true if the BID's parent namespace bytes match one of the UUID_NAMESPACE_* contant's
    /// namespace bytes. User files must not use BIDs in this namespace - they are reserved for
    /// system use.
    ///
    /// This works because:
    ///
    /// - All system BIDs (API versions, href tracking, etc.) are derived from
    ///   one of the UUID_NAMESPACE_* contstants
    ///
    /// - When creating BIDs via `Bid::new()` or similar, the parent's namespace becomes the child's
    ///   parent_namespace_bytes (octets 10-15)
    ///
    /// - We check if those bytes match the Buildonomy namespace (octets 10-15 of
    ///   UUID_NAMESPACE_BUILDONOMY)
    ///
    /// - User-generated BIDs will have different parent namespace bytes
    pub fn is_reserved(&self) -> bool {
        let namespace = self.parent_bref();
        [
            buildonomy_namespace().bref(),
            asset_namespace().bref(),
            href_namespace().bref(),
            buildonomy_namespace().parent_bref(),
            asset_namespace().parent_bref(),
            href_namespace().parent_bref(),
        ]
        .contains(&namespace)
            || self.is_nil()
    }

    /// Display the most significant 20 bytes as a UUID-encoded string, removing the bytes encoding
    /// the parent namespace.
    pub fn display_no_namespace(&self) -> String {
        self.0.as_simple().encode_lower(&mut Uuid::encode_buffer())[..BREF_IDX_START].to_string()
    }

    /// Return the least significant 6 bytes of the Bid's UUID buffer. Per UUIDv7 format and BID
    /// construction, these bits work as a key to the identity of the BID for the generating source
    /// (parent) of this id.
    pub fn parent_bref(&self) -> Bref {
        // We can unwrap because we know that UUIDs will have 16 bytes
        let mut arr = [0u8; 6];
        arr.copy_from_slice(&self.0.as_bytes()[10..16]);
        Bref(arr)
    }

    /// Generate a parent namespace from this ID, for use as the source context when generating
    /// another BID, or for determining whether this BID is the source context for a pre-existing
    /// BID.
    pub fn bref(&self) -> Bref {
        generate_namespace(self).parent_bref()
    }

    /// Generate a filter function to determine whether the input's [Bid::parent_bref] matche
    /// this object's [Bid::bref].
    pub fn is_parent_filter<U>(&self) -> impl Fn(&U) -> bool
    where
        U: AsRef<Bid>,
    {
        let namespace = self.bref();
        move |id: &U| id.as_ref().parent_bref() == namespace
    }
}

impl Default for Bid {
    fn default() -> Self {
        Bid::new(Bid::nil())
    }
}

impl AsRef<Uuid> for Bid {
    fn as_ref(&self) -> &Uuid {
        &self.0
    }
}

impl AsRef<Bid> for Bid {
    fn as_ref(&self) -> &Bid {
        self
    }
}

impl From<Uuid> for Bid {
    fn from(id: Uuid) -> Self {
        Bid(id)
    }
}

impl TryFrom<&[u8]> for Bid {
    type Error = BuildonomyError;

    fn try_from(blob: &[u8]) -> Result<Self, Self::Error> {
        Ok(Bid(Uuid::from_slice(blob)?))
    }
}

impl TryFrom<&str> for Bid {
    type Error = BuildonomyError;

    fn try_from(string: &str) -> Result<Self, Self::Error> {
        Ok(Bid(Uuid::parse_str(string)?))
    }
}

impl Display for Bid {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.0.hyphenated().encode_lower(&mut Uuid::encode_buffer())
        )
    }
}

impl From<&Bid> for String {
    fn from(val: &Bid) -> Self {
        format!("{val}")
    }
}

impl From<Bid> for String {
    fn from(val: Bid) -> Self {
        format!("{val}")
    }
}

const BREF_IDX_START: usize = 20;
/// Belief reference
///
/// The least significant 6 bytes taken from generate_namespace(reference Bid)
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Bref([u8; 6]);

impl Bref {
    pub fn is_default(&self) -> bool {
        *self == Bid::nil().bref()
    }

    pub fn bytes(&self) -> &[u8; 6] {
        &self.0
    }
}

impl fmt::Debug for Bref {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bref({})", self)
    }
}

impl Display for Bref {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for byte in &self.0 {
            write!(f, "{:02x}", byte)?;
        }
        Ok(())
    }
}

impl Default for Bref {
    fn default() -> Self {
        Bid::nil().bref()
    }
}

impl Hash for Bref {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl Serialize for Bref {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Bref {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        if bytes.len() != 6 {
            return Err(serde::de::Error::custom("expected 6 bytes"));
        }
        Ok(Bref(bytes.try_into().unwrap()))
    }
}

uniffi::custom_type!(Bref, String, {
    try_lift: |val| Ok(Bref::try_from(val.as_ref())?),
    lower: |obj| obj.to_string()
});

impl TryFrom<&str> for Bref {
    type Error = BuildonomyError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let bytes = hex::decode(s).map_err(|e| {
            BuildonomyError::Serialization(format!("bref deserialization failed: {}", e))
        })?;
        if bytes.len() != 6 {
            return Err(BuildonomyError::Serialization(format!(
                "bref invalid length. expected 6 got {}",
                bytes.len()
            )));
        }
        Ok(Bref(bytes.try_into().unwrap()))
    }
}

/// [BeliefKind] enumerates all available [BeliefNode] object types per this core api version. Each
/// [BeliefNode] contains an [EnumSet] of these options, in order to designate it's functionality
/// and available operations within a [crate::beliefbase::BeliefBase].
#[derive(
    Debug, Default, Serialize, Deserialize, PartialOrd, Ord, Hash, EnumSetType, uniffi::Enum,
)]
#[enumset(repr = "u32")]
pub enum BeliefKind {
    /// A Buildonomy API node serving as an anchor point for a specific schema version or
    /// implementation. Multiple API nodes can coexist in a BeliefBase, each representing different
    /// schema versions or alternative implementations. All nodes in a valid subgraph must have a
    /// path (via Subsection relations) to at least one API node, which serves as the root of that
    /// subgraph's hierarchy. Network nodes connected to an API represent content representable
    /// at that API's functionality level.
    API,
    /// A repository/directory of beliefs
    Network,
    /// A method to manipulate perceived context
    Action,
    /// A method to abstractly measure/describe driving intentions
    Core,
    /// A way to name a perceptible recurring phenomenon
    #[default]
    Symbol,
    /// A Handle to source material that encodes one or more beliefs
    Document,
    /// Denotes that the Bid wraps an external reference -- it is a link to a source we don't have
    /// native parsing capability for (no read/write access, binary format, external api, etc.).
    External,
    /// Marks a node whose relations are partially loaded, enabling partial hypergraph loading while
    /// maintaining structural integrity. When a node has BeliefKind::Trace, it signals that the
    /// node exists and can be referenced, but its relations may be incomplete for the current query
    /// scope. This allows query results to include referenced nodes (e.g., as edge targets) without
    /// loading their full relationship set, which is essential for satisfying path invariants while
    /// avoiding loading the entire graph. The balance mechanism uses Trace to identify nodes
    /// needing additional queries. During union operations, Trace is removed when a complete
    /// relation set for that node is merged in. Trace nodes enable querying subgraphs while
    /// maintaining valid connections to the unloaded portions of the hypergraph.
    Trace,
}

impl Display for BeliefKind {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BeliefKindSet(pub EnumSet<BeliefKind>);
// Use `Uuid` as a custom type, with `String` as the Builtin
uniffi::custom_type!(BeliefKindSet, u64, {
    remote,
    try_lift: |val| Ok(BeliefKindSet(EnumSet::from_u64(val))),
    lower: |obj| obj.0.as_u64()
});

impl BeliefKindSet {
    /// Defines whether this node is colored as part of another document (is_anchor == true), or is
    /// a standalone document.
    pub fn is_anchor(&self) -> bool {
        self.0
            .intersection(BeliefKind::API | BeliefKind::Network | BeliefKind::Document)
            .is_empty()
    }

    pub fn is_document(&self) -> bool {
        !self.is_anchor()
    }

    pub fn is_network(&self) -> bool {
        !self
            .0
            .intersection(BeliefKind::API | BeliefKind::Network)
            .is_empty()
    }

    /// Defines if this node is colored as containing complete content and relationships
    pub fn is_complete(&self) -> bool {
        !self.0.contains(BeliefKind::Trace)
    }

    /// Defines if this node is colored as external to our read/write authority
    pub fn is_external(&self) -> bool {
        self.0.contains(BeliefKind::External)
    }
}

impl Deref for BeliefKindSet {
    type Target = EnumSet<BeliefKind>;
    fn deref(&self) -> &EnumSet<BeliefKind> {
        &self.0
    }
}

impl DerefMut for BeliefKindSet {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<EnumSet<BeliefKind>> for BeliefKindSet {
    fn from(kind: EnumSet<BeliefKind>) -> Self {
        BeliefKindSet(kind)
    }
}

impl From<BeliefKind> for BeliefKindSet {
    fn from(kind: BeliefKind) -> Self {
        BeliefKindSet(EnumSet::only(kind))
    }
}

impl Display for BeliefKindSet {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// [Weight] holds the data for a single relationship type within a `WeightSet`.
/// All relationship metadata is stored in the payload table, including sort order via WEIGHT_SORT_KEY.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Weight {
    /// An arbitrary data payload for the relationship, represented as a TOML table.
    /// Can store metadata like ownership, sort order, intensity values, notes, etc.
    /// Use WEIGHT_SORT_KEY for ordering, WEIGHT_OWNED_BY for ownership, WEIGHT_DOC_PATH for paths.
    #[serde(flatten)]
    pub payload: Table,
}

/// Key for marking edge ownership in Weight payload.
/// When "owned_by" is "source", the source node owns the edge (e.g., parent_connections).
/// When "owned_by" is "sink" or absent, the sink node owns the edge (default behavior).
pub const WEIGHT_OWNED_BY: &str = "owned_by";

/// Key for storing sort/index value in Weight payload (typically for Subsection relationships)
pub const WEIGHT_SORT_KEY: &str = "sort_key";

/// Key for storing document path in Weight payload (deprecated - use WEIGHT_DOC_PATHS)
#[deprecated(since = "0.1.0", note = "Use WEIGHT_DOC_PATHS instead")]
pub const WEIGHT_DOC_PATH: &str = "doc_path";

/// Key for storing document paths in Weight payload (supports multiple paths per relation)
pub const WEIGHT_DOC_PATHS: &str = "doc_paths";

impl Weight {
    pub fn full() -> Weight {
        let mut weight = Weight {
            payload: Table::new(),
        };
        weight.set(WEIGHT_SORT_KEY, u16::MAX).ok();
        weight
    }

    /// Get a typed value from the payload by key
    pub fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.payload
            .get(key)
            .and_then(|v| v.clone().try_into().ok())
    }

    /// Set a key-value pair in the payload, creating the table if it doesn't exist
    pub fn set<T: serde::Serialize>(
        &mut self,
        key: &str,
        value: T,
    ) -> Result<(), toml::ser::Error> {
        let value_toml = toml::Value::try_from(value)?;
        self.payload.insert(key.to_string(), value_toml);
        Ok(())
    }

    /// Check if payload contains a key
    pub fn contains_key(&self, key: &str) -> bool {
        self.payload.contains_key(key)
    }

    /// Get document paths with backward compatibility. Tries [WEIGHT_DOC_PATHS] first
    /// (`Vec<String>`), falls back to [WEIGHT_DOC_PATH] (String). Returns empty vec if neither is
    /// present.
    pub fn get_doc_paths(&self) -> Vec<String> {
        // Try new format first
        if let Some(paths) = self.get::<Vec<String>>(WEIGHT_DOC_PATHS) {
            return paths;
        }

        // Fall back to old format
        #[allow(deprecated)]
        if let Some(path) = self.get::<String>(WEIGHT_DOC_PATH) {
            return vec![path];
        }

        vec![]
    }

    /// Set document paths (always uses new WEIGHT_DOC_PATHS format).
    /// Warns if setting more than 1 path (unusual case).
    pub fn set_doc_paths(&mut self, paths: Vec<String>) -> Result<(), toml::ser::Error> {
        if paths.len() > 1 {
            tracing::warn!(
                "Setting {} paths for single relation (expected 1). Paths: {:?}",
                paths.len(),
                paths
            );
        }
        self.set(WEIGHT_DOC_PATHS, paths)
    }
}

impl Hash for Weight {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Hash based on sort_key if present, otherwise hash the payload
        let sort_key: Option<u16> = self.get(WEIGHT_SORT_KEY);
        sort_key.hash(state);
    }
}

impl PartialEq for Weight {
    fn eq(&self, other: &Self) -> bool {
        let self_sort: Option<u16> = self.get(WEIGHT_SORT_KEY);
        let other_sort: Option<u16> = other.get(WEIGHT_SORT_KEY);
        self_sort == other_sort
    }
}

impl Eq for Weight {}

impl PartialOrd for Weight {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Weight {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let self_sort: Option<u16> = self.get(WEIGHT_SORT_KEY);
        let other_sort: Option<u16> = other.get(WEIGHT_SORT_KEY);
        self_sort.cmp(&other_sort)
    }
}

/// [WeightKind] identifies what type of node to node relationship an edge represents. Each
/// [crate::beliefbase::BidGraph] represents a hypergraph of these relationship types.
///
/// **Architecture Note (Advisory Council 2025-11-19):** WeightKind is infrastructure-only,
/// carrying NO semantic payload. All semantic information is stored in the Weight.payload field:
/// - For Pragmatic edges: `EnumSet<PragmaticKind> + EnumSet<MotivationDimension>`
/// - For Epistemic edges: dependency metadata, confidence scores
/// - For Subsection edges: section numbering, heading text
///
/// This separation enables clean separation of graph algorithms from domain semantics.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, uniffi::Enum,
)]
pub enum WeightKind {
    Epistemic, // Knowledge dependencies
    Section,   // Document structure
    Pragmatic, // Action/being relationships
}

/// [PragmaticKind] defines semantic kinds for Pragmatic edges in the intention lattice.
/// Multiple kinds can be true simultaneously (stored as EnumSet in Weight.payload).
///
/// These are inferred from procedure structure during schema parsing or explicitly declared
/// in parent_connections. See design/intention_lattice.md for full semantics.
#[derive(EnumSetType, Debug, Serialize, Deserialize)]
#[enumset(serialize_repr = "list")]
pub enum PragmaticKind {
    Constitutive, // Identity-maintaining: action IS the aspiration embodied
    Instrumental, // Goal-achieving: action serves the aspiration as a means
    Expressive,   // Value-expressing: action manifests the aspiration symbolically
    Exploratory,  // Uncertainty-reducing: investigating the meaning of this relationship
}

/// [MotivationDimension] defines which Self-Determination Theory (SDT) dimensions to track
/// for a practice. This is configuration only—actual predicted/observed values are stored
/// privately in practice_statistics (see procedure_engine.md).
///
/// Multiple dimensions can be tracked simultaneously (stored as EnumSet in Weight.payload).
#[derive(EnumSetType, Debug, Serialize, Deserialize)]
#[enumset(serialize_repr = "list")]
pub enum MotivationDimension {
    IntrinsicReward, // Track intrinsic enjoyment/interest
    Autonomous,      // Track sense of choice/volition (vs. external pressure)
    ShouldPressure,  // Track internalized obligation ('should' energy)
    Efficacy,        // Track perceived competence/effectiveness
    Relatedness,     // Track social connection/belonging dimension
}

impl WeightKind {
    pub fn all() -> &'static [WeightKind] {
        &[
            WeightKind::Epistemic,
            WeightKind::Section,
            WeightKind::Pragmatic,
        ]
    }
}

impl Display for WeightKind {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl From<WeightKind> for u32 {
    fn from(src: WeightKind) -> u32 {
        match src {
            WeightKind::Epistemic => 0u32,
            WeightKind::Section => u32::from(u16::MAX),
            WeightKind::Pragmatic => 2 * u32::from(u16::MAX),
        }
    }
}

impl From<&WeightKind> for u32 {
    fn from(src: &WeightKind) -> u32 {
        match src {
            WeightKind::Epistemic => 0u32,
            WeightKind::Section => u32::from(u16::MAX),
            WeightKind::Pragmatic => 2 * u32::from(u16::MAX),
        }
    }
}

impl TryFrom<&str> for WeightKind {
    type Error = BuildonomyError;

    fn try_from(src: &str) -> Result<WeightKind, BuildonomyError> {
        match &src.to_lowercase()[..] {
            "epistemic" => Ok(WeightKind::Epistemic),
            "subsection" => Ok(WeightKind::Section),
            "pragmatic" => Ok(WeightKind::Pragmatic),
            _ => Err(BuildonomyError::Custom(format!(
                "Invalid str for WeightKind. Received {src}. Valid options: epistemic, subsection, pragmatic"
            ))),
        }
    }
}

impl TryFrom<u32> for WeightKind {
    type Error = BuildonomyError;

    fn try_from(src: u32) -> Result<WeightKind, BuildonomyError> {
        match src {
            0..=255 => Ok(WeightKind::Epistemic),
            256..=511 => Ok(WeightKind::Section),
            512..=767 => Ok(WeightKind::Pragmatic),
            _ => Err(BuildonomyError::Custom(format!(
                "Invalid u32 for WeightKind. Max allowed value is 767. Received {src}"
            ))),
        }
    }
}

use std::collections::BTreeMap;

/// [WeightSet] is the edge data structure used within a [crate::beliefbase::BidGraph] to represent the full
/// [crate::beliefbase::BeliefBase] hypergraph within a single graph structure.
///
/// WeightSet methods provide convenience functions for extracting and comparing [WeightKind]
/// specific measures.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WeightSet {
    pub weights: BTreeMap<WeightKind, Weight>,
}

uniffi::custom_type!(WeightSet, String, {
    try_lift: |val: String| -> Result<WeightSet, BuildonomyError> {
        Ok(toml::from_str(&val)?)
    },
    lower: |obj: WeightSet| -> String {
        toml::to_string(&obj).unwrap_or_default()
    },
});

impl WeightSet {
    /// Generate a new weightset with all the weights from lhs and rhs. When there is an overlap in
    /// weights, rhs take precidence and overwrite values from lhs.
    pub fn union(&self, rhs: &Self) -> Self {
        let mut new_weights = self.weights.clone();
        for (kind, weight) in rhs.weights.iter() {
            new_weights.insert(*kind, weight.clone());
        }
        Self {
            weights: new_weights,
        }
    }

    /// Generate a new weightset with all the weights in lhs and rhs. The actual weight value is
    /// taken from rhs.
    pub fn intersection(&self, rhs: &Self) -> Self {
        let mut new_weights = BTreeMap::new();
        for (kind, weight) in self.weights.iter() {
            if rhs.weights.contains_key(kind) {
                new_weights.insert(*kind, weight.clone());
            }
        }
        Self {
            weights: new_weights,
        }
    }

    pub fn get(&self, kind: &WeightKind) -> Option<&Weight> {
        self.weights.get(kind)
    }

    pub fn set(&mut self, kind: WeightKind, weight: Weight) {
        self.weights.insert(kind, weight);
    }

    pub fn remove(&mut self, kind: &WeightKind) -> Option<Weight> {
        self.weights.remove(kind)
    }

    pub fn is_empty(&self) -> bool {
        self.weights.is_empty()
    }

    pub fn difference(&self, rhs: &Self) -> Self {
        let mut new_weights = BTreeMap::new();
        for (kind, weight) in self.weights.iter() {
            if !rhs.weights.contains_key(kind) {
                new_weights.insert(*kind, weight.clone());
            }
        }
        Self {
            weights: new_weights,
        }
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn full() -> Self {
        let mut weights = BTreeMap::new();
        weights.insert(WeightKind::Epistemic, Weight::full());
        weights.insert(WeightKind::Section, Weight::full());
        weights.insert(WeightKind::Pragmatic, Weight::full());
        Self { weights }
    }
}

impl From<WeightKind> for WeightSet {
    fn from(kind: WeightKind) -> Self {
        let mut weights = BTreeMap::new();
        weights.insert(
            kind,
            Weight {
                payload: Table::new(),
            },
        );
        Self { weights }
    }
}

impl IntoIterator for WeightSet {
    type Item = (WeightKind, Weight);
    type IntoIter = std::collections::btree_map::IntoIter<WeightKind, Weight>;

    fn into_iter(self) -> Self::IntoIter {
        self.weights.into_iter()
    }
}

impl<'a> IntoIterator for &'a WeightSet {
    type Item = (&'a WeightKind, &'a Weight);
    type IntoIter = std::collections::btree_map::Iter<'a, WeightKind, Weight>;

    fn into_iter(self) -> Self::IntoIter {
        self.weights.iter()
    }
}

/// Acts as a reference-to and configuration-of an actionable element within a
/// [crate::beliefbase::BeliefBase]. [BeliefNode]s are the nodes (duh) of a Network.
#[derive(Debug, Default, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct BeliefNode {
    pub bid: Bid,
    #[serde(with = "enumset_list")]
    pub kind: BeliefKindSet,
    pub title: String,
    pub schema: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Table::is_empty")]
    pub payload: Table,
    /// Optional semantic identifier from TOML schema (e.g., "asp_sarah_embodiment_rest")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl Hash for BeliefNode {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Hash based on bid, two nodes with the same bid _should_ be the same node
        self.bid.hash(state);
    }
}

impl PartialEq for BeliefNode {
    fn eq(&self, other: &Self) -> bool {
        self.bid == other.bid
            && self.kind == other.kind
            && self.title == other.title
            && self.schema == other.schema
            && self
                .payload
                .iter()
                .zip(other.payload.iter())
                .all(|(a, b)| a == b)
            && self.id == other.id
    }
}

impl Eq for BeliefNode {}

impl BeliefNode {
    /// Generate a unique node to that represents the API version of this buildonomy core library.
    /// Relating the api_state node to [BeliefKind::Network] nodes denotes the API format that
    /// network structure implements.
    pub fn api_state() -> BeliefNode {
        let mut table = Table::new();
        table.insert(
            "package".to_string(),
            Value::String(env!("CARGO_PKG_NAME").to_string()),
        );
        table.insert(
            "version".to_string(),
            Value::String(env!("CARGO_PKG_VERSION").to_string()),
        );
        table.insert(
            "authors".to_string(),
            Value::String(env!("CARGO_PKG_AUTHORS").to_string()),
        );
        table.insert(
            "repository".to_string(),
            Value::String("https:://gitlab.com/buildonomy/noet".to_string()),
        );
        table.insert(
            "license".to_string(),
            Value::String("UNLICENSED".to_string()),
        );
        BeliefNode {
            bid: buildonomy_api_bid(env!("CARGO_PKG_VERSION")),
            title: format!("Buildonomy API v{}", env!("CARGO_PKG_VERSION")),
            schema: Some("api".to_string()),
            payload: table,
            // API node is _always_ also a Trace, as we never can assume we have all api relations
            kind: BeliefKindSet(BeliefKind::API | BeliefKind::Trace),
            id: Some("buildonomy_api".to_string()),
        }
    }

    /// Generate a unique node to that represents the API version of this buildonomy core library.
    /// Relating the api_state node to [BeliefKind::Network] nodes denotes the API format that
    /// network structure implements.
    pub fn href_network() -> BeliefNode {
        let mut table = Table::new();
        table.insert(
            "api".to_string(),
            Value::String(buildonomy_namespace().to_string()),
        );
        BeliefNode {
            bid: href_namespace(),
            title: format!(
                "Buildonomy href tracking network v{}",
                env!("CARGO_PKG_VERSION")
            ),
            schema: Some("api".to_string()),
            payload: table,
            // API node is _always_ also a Trace, as we never can assume we have all api relations
            kind: BeliefKindSet(BeliefKind::Network | BeliefKind::Trace),
            id: Some("buildonomy_href_network".to_string()),
        }
    }

    /// Creates a BeliefNode for the asset tracking network
    pub fn asset_network() -> BeliefNode {
        let mut table = Table::new();
        table.insert(
            "api".to_string(),
            Value::String(buildonomy_namespace().to_string()),
        );
        BeliefNode {
            bid: asset_namespace(),
            title: format!(
                "Buildonomy asset tracking network v{}",
                env!("CARGO_PKG_VERSION")
            ),
            schema: Some("api".to_string()),
            payload: table,
            // Asset network is always a Trace as we track external resources
            kind: BeliefKindSet(BeliefKind::Network | BeliefKind::Trace),
            id: Some("buildonomy_asset_network".to_string()),
        }
    }

    pub fn unknown(bid: Bid) -> BeliefNode {
        BeliefNode {
            bid,
            ..Default::default()
        }
    }

    pub fn display_title(&self) -> String {
        match self.title.is_empty() {
            true => self.bid.bref().to_string(),
            false => self.title.to_string(),
        }
    }

    pub fn id(&self) -> String {
        self.id
            .clone()
            .or_else(|| {
                if !self.title.is_empty() {
                    Some(to_anchor(&self.title))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                if !self.bid.is_nil() {
                    self.bid.bref().to_string()
                } else {
                    String::default()
                }
            })
    }

    /// Generate all valid hrefs per NodeKey::from_str parsing definition with optional namespace
    pub fn keys(
        &self,
        maybe_ns: Option<Bid>,
        maybe_parent: Option<Bid>,
        bs: &BeliefBase,
    ) -> Vec<NodeKey> {
        let ns = maybe_ns.unwrap_or(Bid::nil());
        let net = ns.bref();
        let mut ids = Vec::default();
        if self.bid != Bid::nil() {
            ids.push(NodeKey::Bid { bid: self.bid });
            ids.push(NodeKey::Bref {
                bref: self.bid.bref(),
            });
        }
        let id = self.id();
        if !id.is_empty() {
            ids.push(NodeKey::Id { net, id });
        }
        if let Some(net_pm) = bs.paths().get_map(&ns.bref()) {
            if self.bid != Bid::nil() {
                if let Some((_bid_home_net, ns_relative_path, _order)) =
                    net_pm.path(&self.bid, &bs.paths())
                {
                    ids.push(NodeKey::Path {
                        net,
                        path: ns_relative_path,
                    })
                }
            }
            if let (Some(parent), false, false) =
                (maybe_parent, self.title.is_empty(), self.kind.is_document())
            {
                if let Some((_parent_home_net, ns_relative_parent_path, _order)) =
                    net_pm.path(&parent, &bs.paths())
                {
                    let ns_path_ap = AnchorPath::from(&ns_relative_parent_path);
                    let path = ns_path_ap.join(as_anchor(&self.title));
                    if path != ns_relative_parent_path {
                        ids.push(NodeKey::Path { net, path })
                    }
                }
            }
        }

        ids
    }

    pub fn merge(&mut self, rhs: &BeliefNode) -> bool {
        let mut changed = false;
        if self.bid != rhs.bid {
            self.bid = rhs.bid;
            changed = true;
        }
        if self.title != rhs.title {
            self.title = rhs.title.clone();
            changed = true;
        }
        let mut merged_kind = self.kind.union(rhs.kind.0);
        if !BeliefKindSet::from(merged_kind).is_complete()
            && (self.kind.is_complete() || rhs.kind.is_complete())
        {
            merged_kind.remove(BeliefKind::Trace);
        };
        if merged_kind != self.kind.0 {
            self.kind = merged_kind.into();
            changed = true;
        }
        if self.schema != rhs.schema {
            self.schema = rhs.schema.clone();
            changed = true;
        }
        let keys = BTreeSet::from_iter(
            self.payload
                .keys()
                .cloned()
                .chain(rhs.payload.keys().cloned()),
        );
        for key in keys.into_iter() {
            match (self.payload.get(&key), rhs.payload.get(&key)) {
                (Some(lhs_value), Some(rhs_value)) => {
                    if lhs_value != rhs_value {
                        changed = true;
                        self.payload.insert(key.clone(), rhs_value.clone());
                    }
                }
                (None, Some(rhs_value)) => {
                    changed = true;
                    self.payload.insert(key.clone(), rhs_value.clone());
                }
                _ => {}
            }
        }
        changed
    }

    pub fn toml(&self) -> String {
        to_string(self).expect("Serialization of BeliefNodes cannot fail")
    }
}

impl Display for BeliefNode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}\n\
             \t:bid:  {}\n\
             \t:kind: {}\n\
             \t:schema: {}\n
             \n\
             \t{}",
            self.title,
            self.bid,
            self.kind,
            self.schema.as_deref().unwrap_or("default"),
            self.payload.to_string().replace("\n", "\n\t")
        )
    }
}

#[cfg(feature = "service")]
impl FromRow<'_, SqliteRow> for BeliefNode {
    fn from_row(row: &SqliteRow) -> sqlx::Result<Self> {
        let kind_u32: u32 = row.try_get("kind")?;
        let bid_str: &str = row.try_get("bid")?;
        let bid = Bid::try_from(bid_str)?;

        debug_assert!(Bref::try_from(row.try_get::<&str, _>("bref")?)? == bid.bref());

        let title_str: &str = row.try_get("title")?;
        let schema_str: Option<&str> = row.try_get("schema")?;
        let maybe_id_str: Option<&str> = row.try_get("id")?;
        let serde_str: &str = row.try_get("payload")?;
        let table = toml::from_str::<Table>(serde_str).map_err(BuildonomyError::from)?;

        Ok(BeliefNode {
            bid,
            kind: EnumSet::from_u32(kind_u32).into(),
            title: title_str.to_string(),
            schema: schema_str.map(|schema| schema.to_string()),
            payload: table,
            id: maybe_id_str.map(|id_str| id_str.to_string()),
        })
    }
}

impl TryFrom<&str> for BeliefNode {
    type Error = BuildonomyError;

    fn try_from(string: &str) -> Result<Self, Self::Error> {
        let node = from_str(string)?;
        Ok(node)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl TryFrom<&ProtoBeliefNode> for BeliefNode {
    type Error = BuildonomyError;

    fn try_from(proto: &ProtoBeliefNode) -> Result<Self, Self::Error> {
        let mut doc = proto.document.clone();
        Ok(BeliefNode {
            bid: doc
                .remove("bid")
                .and_then(|val| val.as_str().map(Bid::try_from))
                .unwrap_or(Ok(Bid::nil()))?,
            title: doc
                .remove("title")
                .and_then(|val| val.as_str().map(|s| s.to_string()))
                .unwrap_or_default(),
            schema: doc
                .remove("schema")
                .and_then(|val| val.as_str().map(|s| s.to_string())),
            id: doc
                .remove("id")
                .and_then(|val| val.as_str().map(|s| s.to_string())),
            payload: from_str(&doc.to_string())?,
            kind: proto.kind.clone(),
        })
    }
}

/// Since UUIDv7 BIDs use a timestamp to generate their most significant bits, Ord for BeliefNode
/// will order the nodes according to the timestamp of when they were generated.
impl Ord for BeliefNode {
    fn cmp(&self, other: &Self) -> Ordering {
        self.bid.cmp(&other.bid)
    }
}

impl PartialOrd for BeliefNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Represents a [crate::beliefbase::BidGraph] edge as a structure suitable for saving into a database table.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BeliefRelation {
    pub source: Bid,
    pub sink: Bid,
    pub weights: WeightSet,
}

/// A reference version of a [BeliefRelation]
#[derive(Debug, Clone)]
pub struct BeliefRefRelation<'a> {
    pub source: &'a Bid,
    pub sink: &'a Bid,
    pub weights: &'a WeightSet,
}

impl<'a> PartialEq for BeliefRefRelation<'a> {
    fn eq(&self, other: &Self) -> bool {
        *self.source == *other.source
            && *self.sink == *other.sink
            && *self.weights == *other.weights
    }
}

impl<'a> Eq for BeliefRefRelation<'a> {}

impl<'a> Ord for BeliefRefRelation<'a> {
    fn cmp(&self, other: &Self) -> Ordering {
        let sink_cmp = self.sink.cmp(other.sink);
        match sink_cmp {
            Ordering::Equal => {
                let source_cmp = self.source.cmp(other.source);
                match source_cmp {
                    Ordering::Equal => self.weights.cmp(other.weights),
                    _ => source_cmp,
                }
            }
            _ => sink_cmp,
        }
    }
}

impl<'a> PartialOrd for BeliefRefRelation<'a> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> From<&'a (Bid, Bid, &'a WeightSet)> for BeliefRefRelation<'a> {
    fn from(edge: &'a (Bid, Bid, &'a WeightSet)) -> Self {
        BeliefRefRelation {
            source: &edge.0,
            sink: &edge.1,
            weights: edge.2,
        }
    }
}

impl<'a> From<&'a BeliefRelation> for BeliefRefRelation<'a> {
    fn from(rel: &'a BeliefRelation) -> Self {
        BeliefRefRelation {
            source: &rel.source,
            sink: &rel.sink,
            weights: &rel.weights,
        }
    }
}

impl From<&BeliefRefRelation<'_>> for BeliefRelation {
    fn from(rel: &BeliefRefRelation) -> Self {
        BeliefRelation {
            source: *rel.source,
            sink: *rel.sink,
            weights: rel.weights.clone(),
        }
    }
}

impl From<BeliefRefRelation<'_>> for BeliefRelation {
    fn from(rel: BeliefRefRelation) -> Self {
        BeliefRelation {
            source: *rel.source,
            sink: *rel.sink,
            weights: rel.weights.clone(),
        }
    }
}

impl<'a> From<&'a (Bid, Bid, &'a WeightSet)> for BeliefRelation {
    fn from(edge: &'a (Bid, Bid, &'a WeightSet)) -> Self {
        BeliefRelation::from(BeliefRefRelation::from(edge))
    }
}

// TODO: Add a `payload` column to the `relations` table in the database schema
// and update this implementation to deserialize it into the `Weight` struct.
#[cfg(feature = "service")]
impl FromRow<'_, SqliteRow> for BeliefRelation {
    fn from_row(row: &SqliteRow) -> sqlx::Result<Self> {
        let source_str: &str = row.try_get("source")?;
        let sink_str: &str = row.try_get("sink")?;
        let mut weights = BTreeMap::new();

        for kind in WeightKind::all() {
            let column_name = format!("{kind:?}").to_lowercase();
            // Try to get JSON string from column and deserialize as Weight
            if let Ok(Some(json_str)) = row.try_get::<Option<String>, &str>(&column_name) {
                if let Ok(weight) = toml::from_str::<Weight>(&json_str) {
                    weights.insert(*kind, weight);
                }
            }
        }

        Ok(BeliefRelation {
            source: Bid::try_from(source_str)?,
            sink: Bid::try_from(sink_str)?,
            weights: WeightSet { weights },
        })
    }
}

impl IntoWeightedEdge<WeightSet> for BeliefRelation {
    type NodeId = Bid;

    fn into_weighted_edge(self) -> (Self::NodeId, Self::NodeId, WeightSet) {
        (self.source, self.sink, self.weights)
    }
}

/// Express the intended participant experience for a BeliefBase rendering.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum RenderMode {
    #[default]
    Execute,
    Edit,
    Presentation,
    Graph,
}

impl Display for RenderMode {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl TryFrom<&str> for RenderMode {
    type Error = BuildonomyError;

    fn try_from(string: &str) -> Result<Self, Self::Error> {
        match string {
            "Edit" => Ok(RenderMode::Edit),
            "Execute" => Ok(RenderMode::Execute),
            "Presentation" => Ok(RenderMode::Presentation),
            "Graph" => Ok(RenderMode::Graph),
            _ => Err(BuildonomyError::Command(format!(
                "Unknown RenderMode '{string}'"
            ))),
        }
    }
}

/// Represents the current state of an `AsRun` procedure execution.
#[derive(Debug, Serialize, Deserialize, PartialOrd, Ord, Hash, EnumSetType, uniffi::Enum)]
#[enumset(repr = "u32")]
pub enum AsRunState {
    Running,
    Failed,
    Redlined,
    Inventory,
}

type AsRunStateSet = EnumSet<AsRunState>;
// Use `Uuid` as a custom type, with `String` as the Builtin
uniffi::custom_type!(AsRunStateSet, u64, {
    remote,
    try_lift: |val| Ok(EnumSet::from_u64(val)),
    lower: |obj| obj.as_u64()
});

impl Display for AsRunState {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

// #[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
// pub struct AsRunHandle {
//     pub hid: Bid,
//     pub path: String,
//     pub proc: Bid,
//     pub version: u32,
// }

/// Represents a running instance of a procedure document.
///
/// This struct captures the full context of a procedure's execution, including
/// the network it belongs to, its path, the specific procedure `Bid`, its
/// content, and its current state. It is used to track the dynamic state of a
/// procedure as a participant interacts with it.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq, uniffi::Record)]
pub struct AsRun {
    pub net: Bid,
    pub doc_path: String,
    pub anchor: Bid,
    pub proc: Bid,
    pub doc: String,
    pub state: EnumSet<AsRunState>,
    pub content: String,
    // pub log: Vec<PerceptionEvent>,
    pub mode: RenderMode,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reserved_namespace_checking() {
        // Test that UUID_NAMESPACE_BUILDONOMY itself is reserved
        let namespace_bid = Bid::from(UUID_NAMESPACE_BUILDONOMY);
        assert!(
            namespace_bid.is_reserved(),
            "UUID_NAMESPACE_BUILDONOMY should be reserved"
        );

        // Test that API BIDs generated via buildonomy_api_bid are reserved
        let api_v0 = buildonomy_api_bid("0.0.0");
        println!("API v0.0.0 BID: {}", api_v0);
        println!("API v0.0.0 namespace: {:?}", api_v0.parent_bref());
        println!("Expected namespace: {}", buildonomy_namespace().bref());
        assert!(api_v0.is_reserved(), "API v0.0.0 BID should be reserved");

        let api_v1 = buildonomy_api_bid("1.0.0");
        assert!(api_v1.is_reserved(), "API v1.0.0 BID should be reserved");

        // Test that href namespace is reserved
        let href_bid = href_namespace();
        assert!(
            href_bid.is_reserved(),
            "href_namespace BID should be reserved"
        );

        // Test that a random BID is NOT reserved
        let random_bid = Bid::new(Bid::nil());
        assert!(
            !random_bid.is_reserved(),
            "Random BID should not be reserved"
        );

        // Test that a user-created BID with different namespace is NOT reserved
        let user_bid = Bid(uuid::Uuid::from_bytes([
            0xa0, 0x65, 0xd8, 0x2c, 0x9d, 0x68, 0x44, 0x70, 0xbe, 0x02, 0x02, 0x8f, 0xb6, 0xc5,
            0x07, 0xc0,
        ]));
        assert!(!user_bid.is_reserved(), "User BID should not be reserved");
    }

    #[test]
    fn test_bid_creation_and_adoption() {
        let parent_bid = Bid::new(Bid::nil());
        let mut child_bid = Bid::default();

        assert_ne!(child_bid.parent_bref(), parent_bid.bref());

        child_bid.adopt_into(&parent_bid);

        assert_eq!(child_bid.parent_bref(), parent_bid.bref());
        assert!(parent_bid.is_parent_filter()(&child_bid));
    }

    #[test]
    fn test_weight_set_operations() {
        let mut ws1 = WeightSet::empty();
        let mut weight1 = Weight {
            payload: Table::new(),
        };
        weight1.set(WEIGHT_SORT_KEY, 1u16).ok();
        ws1.set(WeightKind::Epistemic, weight1);

        let mut table = toml::value::Table::new();
        table.insert(
            WEIGHT_DOC_PATHS.to_string(),
            toml::Value::Array(vec![toml::Value::String("path1".to_string())]),
        );
        let mut weight2 = Weight { payload: table };
        weight2.set(WEIGHT_SORT_KEY, 2u16).ok();
        ws1.set(WeightKind::Section, weight2);

        let mut ws2 = WeightSet::empty();
        let mut weight3 = Weight {
            payload: Table::new(),
        };
        weight3.set(WEIGHT_SORT_KEY, 3u16).ok();
        ws2.set(WeightKind::Epistemic, weight3);

        let mut weight4 = Weight {
            payload: Table::new(),
        };
        weight4.set(WEIGHT_SORT_KEY, 4u16).ok();
        ws2.set(WeightKind::Pragmatic, weight4);

        // Test union
        let union_ws = ws1.union(&ws2);
        assert_eq!(union_ws.weights.len(), 3);
        assert_eq!(
            union_ws
                .get(&WeightKind::Epistemic)
                .unwrap()
                .get::<u16>(WEIGHT_SORT_KEY),
            Some(3)
        ); // ws2 overwrites ws1
        assert_eq!(
            union_ws
                .get(&WeightKind::Section)
                .unwrap()
                .get::<u16>(WEIGHT_SORT_KEY),
            Some(2)
        );
        assert_eq!(
            union_ws
                .get(&WeightKind::Pragmatic)
                .unwrap()
                .get::<u16>(WEIGHT_SORT_KEY),
            Some(4)
        );

        // Test intersection
        let intersection_ws = ws1.intersection(&ws2);
        assert_eq!(intersection_ws.weights.len(), 1);
        assert_eq!(
            intersection_ws
                .get(&WeightKind::Epistemic)
                .unwrap()
                .get::<u16>(WEIGHT_SORT_KEY),
            Some(1)
        );

        // Test difference
        let diff_ws = ws1.difference(&ws2);
        assert_eq!(diff_ws.weights.len(), 1);
        assert!(diff_ws.weights.contains_key(&WeightKind::Section));
        let diff_ws_path = diff_ws
            .weights
            .get(&WeightKind::Section)
            .filter(|w| w.get_doc_paths() == vec!["path1".to_string()]);
        assert!(diff_ws_path.is_some());
    }

    #[test]
    fn test_weight_doc_paths() {
        // Test new format
        let mut weight = Weight::default();
        weight
            .set_doc_paths(vec!["path1".to_string(), "path2".to_string()])
            .unwrap();
        assert_eq!(
            weight.get_doc_paths(),
            vec!["path1".to_string(), "path2".to_string()]
        );

        // Test backward compatibility - reading old format
        let mut old_weight = Weight::default();
        #[allow(deprecated)]
        old_weight
            .set(WEIGHT_DOC_PATH, "old_path".to_string())
            .unwrap();
        assert_eq!(old_weight.get_doc_paths(), vec!["old_path".to_string()]);

        // Test empty case
        let empty_weight = Weight::default();
        assert_eq!(empty_weight.get_doc_paths(), Vec::<String>::new());

        // Test single path (common case - should not warn in test, but would in production)
        let mut single_weight = Weight::default();
        single_weight
            .set_doc_paths(vec!["single_path".to_string()])
            .unwrap();
        assert_eq!(
            single_weight.get_doc_paths(),
            vec!["single_path".to_string()]
        );
    }

    #[test]
    fn test_weight_doc_paths_multiple() {
        // Test setting multiple paths (e.g., for assets referenced from multiple locations)
        let mut weight = Weight::default();
        let paths = vec![
            "images/logo.png".to_string(),
            "guide/../images/logo.png".to_string(),
            "docs/assets/logo.png".to_string(),
        ];
        weight.set_doc_paths(paths.clone()).unwrap();
        assert_eq!(weight.get_doc_paths(), paths);

        // Test that get_doc_paths returns empty vec when no paths are set
        let empty_weight = Weight::default();
        assert!(empty_weight.get_doc_paths().is_empty());

        // Test backward compatibility: old format should convert to vec
        let mut old_format_weight = Weight::default();
        #[allow(deprecated)]
        old_format_weight
            .set(WEIGHT_DOC_PATH, "old/path.md".to_string())
            .unwrap();
        assert_eq!(
            old_format_weight.get_doc_paths(),
            vec!["old/path.md".to_string()]
        );

        // Test that new format takes precedence over old format if both exist
        let mut mixed_weight = Weight::default();
        #[allow(deprecated)]
        mixed_weight
            .set(WEIGHT_DOC_PATH, "old_path.md".to_string())
            .unwrap();
        mixed_weight
            .set_doc_paths(vec!["new_path1.md".to_string(), "new_path2.md".to_string()])
            .unwrap();
        assert_eq!(
            mixed_weight.get_doc_paths(),
            vec!["new_path1.md".to_string(), "new_path2.md".to_string()]
        );
    }
}
