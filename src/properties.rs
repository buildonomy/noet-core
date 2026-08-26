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
    lower: |obj| obj.hyphenated().encode_lower(&mut Uuid::encode_buffer()).to_string()
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
use crate::codec::belief_ir::IRNode;

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

/// The 'codec' namespace UUID. Used by codec-registered secondary index namespaces
/// (e.g. C++ include paths, slug resolution). First byte is `0xff` so codec namespace
/// BIDs sort after all content network BIDs in BTreeSet iteration order — this ensures
/// `partition_graph`'s `or_insert` semantics always assign nodes to content networks first.
pub const UUID_NAMESPACE_CODEC: Uuid = Uuid::from_bytes([
    0xff, 0x3d, 0x21, 0x54, 0xc0, 0xa9, 0x43, 0x7b, 0x93, 0x24, 0x5f, 0x62, 0xad, 0xeb, 0x9a, 0x44,
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

/// Root BID for all codec-registered secondary index namespaces.
pub fn codec_namespace_root() -> Bid {
    Bid::from(UUID_NAMESPACE_CODEC)
}

/// All reserved/const namespaces. Used by `is_reserved()` and anywhere
/// the full set of system namespaces is needed.
pub fn const_namespaces() -> [Bid; 4] {
    [
        buildonomy_namespace(),
        href_namespace(),
        asset_namespace(),
        codec_namespace_root(),
    ]
}

/// Namespaces that track external content anchored to the parsed repo (hrefs, assets).
/// Excludes `buildonomy_namespace` which is structural/API — its paths are not
/// anchored to the parsed root in the same way.
pub fn content_namespaces() -> [Bid; 2] {
    [href_namespace(), asset_namespace()]
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

/// Generate a deterministic BID for an external href node from its URL address.
///
/// The BID is derived from the URL string itself (the address), NOT from the fetched content
/// at that address. This is an intentional design choice: fetching remote content is expensive
/// and unstable, so URL identity is used as the stable hash surface instead. Two href nodes
/// pointing to the same URL will always get the same BID, enabling deduplication across parses
/// without requiring a network fetch.
pub fn buildonomy_href_bid(hash_str: &str) -> Bid {
    // Generate a UUID v5 from the URL string for deterministic, address-based identity
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
        const_namespaces()
            .iter()
            .any(|ns| namespace == ns.bref() || namespace == ns.parent_bref())
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

    /// Derive a deterministic BID for a codec-registered secondary index namespace.
    ///
    /// The term is normalized via `to_anchor()` before hashing to prevent whitespace,
    /// casing, or separator differences from producing different BIDs for the same
    /// logical namespace.
    ///
    /// This is a pure function — callable anywhere with no state. Codec instances
    /// (which are ephemeral, created fresh per file) use a hardcoded string constant
    /// and call this to derive the namespace bref for annotating IRNode fields and
    /// edge keys.
    pub fn codec_namespace(term: &str) -> Bid {
        let normalized = to_anchor(term);
        let uuid = Uuid::new_v5(&UUID_NAMESPACE_CODEC, normalized.as_bytes());
        // Stamp octets 10-15 with the codec namespace root's bref so that
        // is_reserved() detects these BIDs as reserved.
        let mut bytes = *uuid.as_bytes();
        bytes[10..16].copy_from_slice(codec_namespace_root().bref().bytes());
        Bid(Uuid::from_bytes(bytes))
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
    /// Marks a node whose relations are partially loaded, enabling partial multigraph loading while
    /// maintaining structural integrity. When a node has BeliefKind::Trace, it signals that the
    /// node exists and can be referenced, but its relations may be incomplete for the current query
    /// scope. This allows query results to include referenced nodes (e.g., as edge targets) without
    /// loading their full relationship set, which is essential for satisfying path invariants while
    /// avoiding loading the entire graph. The balance mechanism uses Trace to identify nodes
    /// needing additional queries. During union operations, Trace is removed when a complete
    /// relation set for that node is merged in. Trace nodes enable querying subgraphs while
    /// maintaining valid connections to the unloaded portions of the multigraph.
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

/// Ownership marker for edge weights.
/// - `"source"`: the source node owns this edge (e.g., parent_connections).
/// - `"sink"` or absent: the sink node owns this edge (default behavior).
/// - Any other string: a bref identifying a third-party owner node (e.g., a section with a
///   `{maps_to}` directive). Used by `compute_diff` to scope GC to the owning section.
pub const WEIGHT_OWNED_BY: &str = "owned_by";

/// Key for storing sort/index value in Weight payload (typically for Subsection relationships)
pub const WEIGHT_SORT_KEY: &str = "sort_key";

/// Key for storing document path in Weight payload (deprecated - use WEIGHT_DOC_PATHS)
#[deprecated(since = "0.1.0", note = "Use WEIGHT_DOC_PATHS instead")]
pub const WEIGHT_DOC_PATH: &str = "doc_path";

/// Key for storing document paths in Weight payload (supports multiple paths per relation)
pub const WEIGHT_DOC_PATHS: &str = "doc_paths";

/// Key for storing the link display text in Weight payload.
/// Set during markdown parse when the author writes custom link text (e.g., `[My Label](target.md)`).
/// Only present when the link text differs from the target node's title.
pub const WEIGHT_LINK_TITLE: &str = "title";

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
    /// Multiple paths are supported for path aliases (e.g. include-convention
    /// paths alongside filesystem paths, or derived header filenames).
    pub fn set_doc_paths(&mut self, paths: Vec<String>) -> Result<(), toml::ser::Error> {
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
/// [crate::beliefbase::BidGraph] represents a multigraph of these relationship types.
///
/// **Architecture Note (Advisory Council 2025-11-19):** WeightKind is infrastructure-only,
/// carrying NO semantic payload. All semantic information is stored in the Weight.payload field:
/// - For Pragmatic edges: `EnumSet<PragmaticKind> + EnumSet<MotivationDimension>`
/// - For Epistemic edges: dependency metadata, confidence scores
/// - For Subsection edges: section numbering, heading text
///
/// This separation enables clean separation of graph algorithms from domain semantics.
#[derive(Debug, Serialize, Deserialize, PartialOrd, Ord, Hash, EnumSetType, uniffi::Enum)]
#[enumset(repr = "u8")]
pub enum WeightKind {
    Section,   // Structural containment (S content)
    Pragmatic, // Procedural/operational relationships (P content)
    Epistemic, // Normative coupling and knowledge dependencies (N content)
}

impl WeightKind {
    pub fn all() -> &'static [WeightKind] {
        &[
            WeightKind::Section,
            WeightKind::Pragmatic,
            WeightKind::Epistemic,
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
            WeightKind::Section => 0u32,
            WeightKind::Pragmatic => u32::from(u16::MAX),
            WeightKind::Epistemic => 2 * u32::from(u16::MAX),
        }
    }
}

impl From<&WeightKind> for u32 {
    fn from(src: &WeightKind) -> u32 {
        match src {
            WeightKind::Section => 0u32,
            WeightKind::Pragmatic => u32::from(u16::MAX),
            WeightKind::Epistemic => 2 * u32::from(u16::MAX),
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
            0..=255 => Ok(WeightKind::Section),
            256..=511 => Ok(WeightKind::Pragmatic),
            512..=767 => Ok(WeightKind::Epistemic),
            _ => Err(BuildonomyError::Custom(format!(
                "Invalid u32 for WeightKind. Max allowed value is 767. Received {src}"
            ))),
        }
    }
}

use std::collections::BTreeMap;

/// [WeightSet] is the edge data structure used within a [crate::beliefbase::BidGraph] to represent the full
/// [crate::beliefbase::BeliefBase] multigraph within a single graph structure.
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
        weights.insert(WeightKind::Section, Weight::full());
        weights.insert(WeightKind::Pragmatic, Weight::full());
        weights.insert(WeightKind::Epistemic, Weight::full());
        Self { weights }
    }

    /// Extract a deterministic sort key for ordering edges.
    ///
    /// Checks each `WeightKind` in `kind_filter` by enum ordinal
    /// (Section, Pragmatic, Epistemic) and returns the first
    /// `WEIGHT_SORT_KEY` found. Returns `u16::MAX` if no matching
    /// kind carries a sort key.
    ///
    /// The enum ordinal reflects structural importance: Section
    /// (document hierarchy) first, then Pragmatic (action/traceability),
    /// then Epistemic (knowledge dependencies).
    pub fn sort_key(&self, kind_filter: &enumset::EnumSet<WeightKind>) -> u16 {
        kind_filter
            .iter()
            .find_map(|kind| {
                self.weights
                    .get(&kind)
                    .and_then(|w| w.get::<u16>(WEIGHT_SORT_KEY))
            })
            .unwrap_or(u16::MAX)
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

/// The identity state of a node's anchor/ID.
///
/// Separates the HTML anchor (document-scoped, derived from heading text) from the
/// network-scoped ID (must be unique within a network).  See ISSUE_75 for motivation.
///
/// Serialized as `Option<String>` for backward compatibility with existing msgpack
/// shards: `Slug` and `Collision` both serialize as absent (`None`), `Explicit`
/// serializes as the string value.
#[derive(Debug, Clone, PartialEq, Eq, Default, uniffi::Enum)]
pub enum NodeId {
    /// No explicit ID.  Title-derived slug is used for both anchor and network ID.
    /// This is the initial state for headings without `{#id}`.
    #[default]
    Slug,
    /// An explicit ID used for both the HTML anchor and network-scoped ID.
    /// Sources: user-authored `{#intro}`, intra-document collision resolution
    /// (slug-N suffix from `md.rs`), or system-assigned bref fallback.
    /// Already normalized via `to_anchor()`.
    Explicit(String),
    /// Inter-doc network-level ID collision detected by FIRST-ONE-WINS.  The
    /// title-derived slug is still used for the HTML anchor and PathMap path
    /// fragment (it is document-unique).  Network-scoped disambiguation uses
    /// `node.bid.bref()` (already on the node; no inner value stored here).
    ///
    /// The inner `String` is the original title-derived slug that caused the
    /// collision, stored so that on reparse we can detect when the collision
    /// has been resolved (e.g. user renamed the heading or added `{#explicit-id}`).
    Collision(String),
}

impl NodeId {
    /// The value to use for the HTML `id` attribute and PathMap path fragment.
    /// Returns the explicit ID string for `Explicit`, or empty for `Slug`/`Collision`
    /// (callers fall through to `to_anchor(title)`).
    pub fn anchor(&self) -> &str {
        match self {
            NodeId::Explicit(s) => s.as_str(),
            NodeId::Slug | NodeId::Collision(_) => "",
        }
    }

    /// Whether this node underwent a network-level inter-doc ID collision.
    pub fn is_collision(&self) -> bool {
        matches!(self, NodeId::Collision(_))
    }

    /// Whether this node has an explicit (user-authored) ID.
    pub fn is_explicit(&self) -> bool {
        matches!(self, NodeId::Explicit(_))
    }
}

/// Prefix used to distinguish `Collision` from `Explicit` in serialized form.
/// IDs are always slugified (lowercase + dashes), so this prefix is unambiguous.
pub(crate) const COLLISION_PREFIX: &str = "@@collision:";

impl Serialize for NodeId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            NodeId::Slug => serializer.serialize_none(),
            NodeId::Collision(slug) => {
                serializer.serialize_some(&format!("{COLLISION_PREFIX}{slug}"))
            }
            NodeId::Explicit(s) => serializer.serialize_some(s),
        }
    }
}

impl<'de> Deserialize<'de> for NodeId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        Ok(match opt {
            None => NodeId::Slug,
            Some(s) if s.is_empty() => NodeId::Slug,
            Some(s) if s.starts_with(COLLISION_PREFIX) => {
                NodeId::Collision(s[COLLISION_PREFIX.len()..].to_string())
            }
            Some(s) => NodeId::Explicit(s),
        })
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeId::Slug => write!(f, "<slug>"),
            NodeId::Explicit(s) => write!(f, "{}", s),
            NodeId::Collision(slug) => write!(f, "<collision:{slug}>"),
        }
    }
}

/// Serde skip predicate for `BeliefNode.id`: skip serialization when Slug
/// (Collision is now serialized with a prefix so it round-trips correctly).
fn skip_node_id(id: &NodeId) -> bool {
    matches!(id, NodeId::Slug)
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
    /// Node identity: explicit user-authored ID, slug-derived (no explicit ID), or
    /// collision (inter-doc slug collision resolved by FIRST-ONE-WINS).
    /// Serialized as `Option<String>` for shard backward compatibility.
    #[serde(skip_serializing_if = "skip_node_id")]
    pub id: NodeId,
    /// Runtime metadata: per-parse annotations such as git status and source backlinks.
    /// Serialized via `toml()` (carried in `BeliefEvent::NodeUpdate`) and persisted in the
    /// DB `metadata` column so it survives the full parse → DB → export → browser round-trip.
    /// Never appears in source files: `generate_source` is driven by the markdown event
    /// stream and `IRNode::as_frontmatter`, neither of which reads `BeliefNode::metadata`.
    /// Included in `PartialEq` so merges and `compute_diff` propagate it correctly.
    #[serde(default)]
    #[serde(skip_serializing_if = "Table::is_empty")]
    pub metadata: Table,
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
            && self.payload == other.payload
            && self.id == other.id
            && self.metadata == other.metadata
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
            // API node is _always_ also a Trace, as we never can assume we have all api relations.
            // External marks it as permanently Trace — no deeper cache fetch will yield a non-Trace
            // version, so cache_fetch accepts it as a valid hit without falling through.
            kind: BeliefKindSet(BeliefKind::API | BeliefKind::External | BeliefKind::Trace),
            id: NodeId::Explicit("buildonomy_api".to_string()),
            metadata: Table::new(),
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
            // Href network is always Trace — it tracks external URLs, never fully parsed.
            // External marks it as permanently Trace so cache_fetch accepts it without
            // falling through to a deeper (always-empty) global_bb fetch.
            kind: BeliefKindSet(BeliefKind::Network | BeliefKind::External | BeliefKind::Trace),
            id: NodeId::Explicit("buildonomy_href_network".to_string()),
            metadata: Table::new(),
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
            // Asset network is always Trace — it tracks external file references, never fully
            // parsed. External marks it as permanently Trace so cache_fetch accepts it without
            // falling through to a deeper (always-empty) global_bb fetch.
            kind: BeliefKindSet(BeliefKind::Network | BeliefKind::External | BeliefKind::Trace),
            id: NodeId::Explicit("buildonomy_asset_network".to_string()),
            metadata: Table::new(),
        }
    }

    /// Creates a BeliefNode for a codec-registered secondary index namespace network.
    ///
    /// The BID is derived from the term via `Bid::codec_namespace(term)`.
    /// These nodes are structural — they serve as PathMap roots for cross-network
    /// lookup indices (e.g. C++ include paths).
    pub fn codec_network(term: &str) -> BeliefNode {
        let bid = Bid::codec_namespace(term);
        let mut table = Table::new();
        table.insert(
            "api".to_string(),
            Value::String(buildonomy_namespace().to_string()),
        );
        BeliefNode {
            bid,
            title: format!("Codec namespace: {term}"),
            schema: Some("api".to_string()),
            payload: table,
            kind: BeliefKindSet(BeliefKind::Network | BeliefKind::External | BeliefKind::Trace),
            id: NodeId::Explicit(format!("buildonomy_codec_{}", to_anchor(term))),
            metadata: Table::new(),
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

    /// Returns the effective ID string for this node.  Fallback chain:
    /// 1. Explicit ID string (for `NodeId::Explicit`)
    /// 2. Title-derived slug via `to_anchor(title)` (for `Slug` and `Collision`)
    /// 3. Bref string (if title is empty)
    /// 4. Empty string (nil BID, no title)
    ///
    /// Note: `Collision` returns the title slug (same as `Slug`) for consistency
    /// with `insert_state`'s key-matching. Use `collision_aware_id()` when you
    /// need the bref-based disambiguated ID for cache lookups.
    pub fn id(&self) -> String {
        match &self.id {
            NodeId::Explicit(s) if !s.is_empty() => s.clone(),
            _ => {
                if !self.title.is_empty() {
                    to_anchor(&self.title)
                } else if !self.bid.is_nil() {
                    self.bid.bref().to_string()
                } else {
                    String::default()
                }
            }
        }
    }

    /// Returns the ID to use for network-scoped cache lookups.
    /// For `Collision` nodes, returns the bref (disambiguated ID) instead of
    /// the colliding slug.  For all other variants, delegates to `id()`.
    pub fn collision_aware_id(&self) -> String {
        match &self.id {
            NodeId::Collision(_) => self.bid.bref().to_string(),
            _ => self.id(),
        }
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
            if content_namespaces().contains(&ns) {
                // Content namespace nodes (href, asset) have ids that are locations
                // (URLs, file paths). These parse as NodeKey::Path via from_str,
                // so generate a Path key so cache lookups match from_str output.
                ids.push(NodeKey::Path { net, path: id });
            } else {
                ids.push(NodeKey::Id { net, id });
            }
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
                    let path: String = ns_path_ap.join(as_anchor(&self.title)).into_string();
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
                (Some(lhs_value), Some(rhs_value)) if lhs_value != rhs_value => {
                    changed = true;
                    self.payload.insert(key.clone(), rhs_value.clone());
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

    /// Render this node's `payload.text` field as HTML.
    ///
    /// Uses `render_markdown_snippet` (canonical parser options + broken
    /// link callback). Returns empty string if no text payload exists.
    pub fn render_text_html(&self) -> String {
        self.payload
            .get("text")
            .and_then(|v| v.as_str())
            .filter(|t| !t.is_empty())
            .map(crate::codec::render_markdown_snippet)
            .unwrap_or_default()
    }

    /// Apply source-file-derived fields from `ir` into `self`, leaving runtime-only
    /// fields (`bid`, `metadata`) untouched.
    ///
    /// `finalize()` in codecs constructs `IRNode`s that reflect what changed in the
    /// source (sections table, payload updates, etc.).  The caller needs to push those
    /// changes into the canonical `BeliefNode` that lives in `doc_bb` without losing
    /// runtime annotations (git status, source backlinks) that were injected by
    /// `push()` and are not stored in source files.
    ///
    /// Fields updated: `kind`, `title`, `schema`, `payload`, `id`.
    /// Fields preserved: `bid`, `metadata`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn apply_source_update(&mut self, ir: &IRNode) -> Result<bool, BuildonomyError> {
        let updated = BeliefNode::try_from(ir)?;
        let mut changed = false;
        if self.kind != updated.kind {
            self.kind = updated.kind;
            changed = true;
        }
        if self.title != updated.title {
            self.title = updated.title;
            changed = true;
        }
        if self.schema != updated.schema {
            self.schema = updated.schema;
            changed = true;
        }
        if self.payload != updated.payload {
            self.payload = updated.payload;
            changed = true;
        }
        if self.id != updated.id {
            self.id = updated.id;
            changed = true;
        }
        Ok(changed)
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
        // metadata omitted from Display: ephemeral, not part of stable node identity
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
        let metadata_str: Option<&str> = row.try_get("metadata")?;
        let metadata = match metadata_str {
            Some(s) if !s.is_empty() => {
                toml::from_str::<Table>(s).map_err(BuildonomyError::from)?
            }
            _ => Table::new(),
        };

        Ok(BeliefNode {
            bid,
            kind: EnumSet::from_u32(kind_u32).into(),
            title: title_str.to_string(),
            schema: schema_str.map(|schema| schema.to_string()),
            payload: table,
            id: match maybe_id_str {
                Some(s) if s.starts_with(COLLISION_PREFIX) => {
                    NodeId::Collision(s[COLLISION_PREFIX.len()..].to_string())
                }
                Some(s) if !s.is_empty() => NodeId::Explicit(s.to_string()),
                _ => NodeId::Slug,
            },
            metadata,
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
impl TryFrom<&IRNode> for BeliefNode {
    type Error = BuildonomyError;

    fn try_from(proto: &IRNode) -> Result<Self, Self::Error> {
        let mut doc = proto.document.clone();
        // `metadata` is runtime-only and must never bleed into `payload`.  Strip it
        // unconditionally here: `TryFrom<&BeliefNode> for IRNode` already removes it from
        // IRNode.document at the conversion boundary, so under normal operation this remove
        // is a no-op.  It remains as a belt-and-suspenders guard against any future
        // propagation path that bypasses that strip.
        doc.remove("metadata");
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
                .and_then(|val| {
                    val.as_str()
                        .map(|s| s.to_string())
                        .or_else(|| val.as_integer().map(|n| n.to_string()))
                        .or_else(|| val.as_float().map(|f| f.to_string()))
                        .or_else(|| Some(format!("{val:?}")))
                })
                .map(NodeId::Explicit)
                .unwrap_or_default(),
            payload: from_str(&doc.to_string())?,
            kind: proto.kind.clone(),
            metadata: Table::new(),
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
    fn test_weight_owned_by_survives_msgpack_roundtrip() {
        // Regression test: WEIGHT_OWNED_BY with a third-party bref must survive
        // rmp_serde serialization/deserialization. This value is used by
        // BeliefContext::declared_edges() and the get_maps_to* MCP tools to
        // identify edges owned by a {maps_to} directive section node.
        let mut ws = WeightSet::from(WeightKind::Pragmatic);
        {
            let weight = ws.weights.get_mut(&WeightKind::Pragmatic).unwrap();
            weight.set(WEIGHT_OWNED_BY, "abc12def3456").unwrap();
            weight.set(WEIGHT_SORT_KEY, 7u16).unwrap();
        }

        // Verify pre-round-trip value is readable.
        let pre: Option<String> = ws
            .weights
            .get(&WeightKind::Pragmatic)
            .unwrap()
            .get(WEIGHT_OWNED_BY);
        assert_eq!(
            pre.as_deref(),
            Some("abc12def3456"),
            "owned_by not set correctly before round-trip"
        );

        // Round-trip through rmp_serde (the same codec used for shard files).
        let encoded = rmp_serde::to_vec_named(&ws).expect("msgpack encode failed");
        let decoded: WeightSet = rmp_serde::from_slice(&encoded).expect("msgpack decode failed");

        let post: Option<String> = decoded
            .weights
            .get(&WeightKind::Pragmatic)
            .unwrap()
            .get(WEIGHT_OWNED_BY);
        assert_eq!(
            post.as_deref(),
            Some("abc12def3456"),
            "WEIGHT_OWNED_BY lost during rmp_serde round-trip — Weight#[serde(flatten)] incompatibility"
        );

        // Also verify sort_key survived.
        let sort_key: Option<u16> = decoded
            .weights
            .get(&WeightKind::Pragmatic)
            .unwrap()
            .get(WEIGHT_SORT_KEY);
        assert_eq!(sort_key, Some(7u16), "sort_key lost during round-trip");
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
    #[test]
    fn test_belief_node_metadata_serde_round_trip_json() {
        // metadata must survive a JSON round-trip (the beliefbase.json → browser path).
        let mut metadata = Table::new();
        let mut git = Table::new();
        git.insert(
            "remote_url".to_string(),
            Value::String("https://github.com/org/repo".to_string()),
        );
        git.insert("branch".to_string(), Value::String("main".to_string()));
        git.insert("dirty".to_string(), Value::Boolean(false));
        metadata.insert("git".to_string(), Value::Table(git));
        metadata.insert(
            "source_url".to_string(),
            Value::String("https://github.com/org/repo/blob/main/docs/guide.md#L42".to_string()),
        );

        let node = BeliefNode {
            bid: Bid::new(Bid::nil()),
            kind: crate::properties::BeliefKind::Document.into(),
            title: "Guide".to_string(),
            schema: None,
            payload: Table::new(),
            id: NodeId::Explicit("guide".to_string()),
            metadata,
        };

        let json = serde_json::to_string(&node).expect("serialize to JSON");
        let restored: BeliefNode = serde_json::from_str(&json).expect("deserialize from JSON");

        assert_eq!(node, restored, "BeliefNode must round-trip through JSON");
        assert_eq!(
            restored.metadata.get("source_url").and_then(|v| v.as_str()),
            Some("https://github.com/org/repo/blob/main/docs/guide.md#L42"),
            "source_url must survive JSON round-trip"
        );
        assert!(
            restored.metadata.get("git").is_some(),
            "git sub-table must survive JSON round-trip"
        );
    }

    #[test]
    fn test_belief_node_metadata_serde_round_trip_toml() {
        // metadata must survive a TOML round-trip (the NodeUpdate event → DB path).
        let mut metadata = Table::new();
        metadata.insert(
            "source_url".to_string(),
            Value::String("https://github.com/org/repo/blob/main/src/lib.rs".to_string()),
        );

        let node = BeliefNode {
            bid: Bid::new(Bid::nil()),
            kind: crate::properties::BeliefKind::Document.into(),
            title: "Lib".to_string(),
            schema: None,
            payload: Table::new(),
            id: NodeId::default(),
            metadata,
        };

        let toml_str = node.toml();
        let restored: BeliefNode =
            toml::from_str(&toml_str).expect("deserialize BeliefNode from TOML");

        assert_eq!(node, restored, "BeliefNode must round-trip through TOML");
        assert_eq!(
            restored.metadata.get("source_url").and_then(|v| v.as_str()),
            Some("https://github.com/org/repo/blob/main/src/lib.rs"),
            "source_url must survive TOML round-trip"
        );
    }

    /// The actual generate_source round-trip test lives in `codec::md::tests` as
    /// `test_metadata_not_in_generate_source`, where MdCodec is available.
    ///
    /// This test previously used `IRNode::try_from(&BeliefNode)` as a proxy for
    /// `generate_source`, but that conversion is not on the generate_source path.
    /// `generate_source` drives exclusively from the markdown event stream
    /// (`MdCodec::current_events`) and never reads `BeliefNode::metadata`, so
    /// metadata exclusion must be verified via a full MdCodec parse + generate_source
    /// round-trip, not via IRNode frontmatter serialization.
    #[test]
    fn test_belief_node_metadata_serde_excludes_metadata_when_empty() {
        // Confirm that an empty metadata table is not serialized (skip_serializing_if).
        let node = BeliefNode {
            bid: Bid::new(Bid::nil()),
            kind: crate::properties::BeliefKind::Document.into(),
            title: "Doc".to_string(),
            schema: None,
            payload: Table::new(),
            id: NodeId::Explicit("doc".to_string()),
            metadata: Table::new(),
        };
        let toml = node.toml();
        assert!(
            !toml.contains("metadata"),
            "empty metadata must be omitted from serialization; got:\n{toml}"
        );
    }

    #[test]
    fn test_codec_namespace_determinism() {
        let bid1 = Bid::codec_namespace("include");
        let bid2 = Bid::codec_namespace("include");
        assert_eq!(bid1, bid2, "Same term must produce same BID");
    }

    #[test]
    fn test_codec_namespace_is_reserved() {
        let bid = Bid::codec_namespace("include");
        assert!(bid.is_reserved(), "Codec namespace BIDs must be reserved");
    }

    #[test]
    fn test_codec_namespace_distinct_from_const_namespaces() {
        let include_bid = Bid::codec_namespace("include");
        assert_ne!(include_bid, href_namespace());
        assert_ne!(include_bid, asset_namespace());
        assert_ne!(include_bid, buildonomy_namespace());
    }

    #[test]
    fn test_codec_namespace_different_terms_produce_different_bids() {
        let include = Bid::codec_namespace("include");
        let slug = Bid::codec_namespace("slug");
        assert_ne!(include, slug);
    }

    #[test]
    fn test_codec_namespace_normalization() {
        // to_anchor normalizes casing and whitespace
        let bid1 = Bid::codec_namespace("Include");
        let bid2 = Bid::codec_namespace("include");
        assert_eq!(bid1, bid2, "Terms should be normalized via to_anchor");
    }

    #[test]
    fn test_codec_network_factory() {
        let node = BeliefNode::codec_network("include");
        assert_eq!(node.bid, Bid::codec_namespace("include"));
        assert!(node.kind.contains(BeliefKind::Network));
        assert!(node.kind.contains(BeliefKind::External));
        assert!(node.kind.contains(BeliefKind::Trace));
    }

    #[test]
    fn test_codec_namespace_sort_order() {
        // Codec namespace root should sort after all other const namespaces
        let codec = codec_namespace_root();
        assert!(codec > buildonomy_namespace());
        assert!(codec > href_namespace());
        assert!(codec > asset_namespace());
    }
}
