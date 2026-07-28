/// Builder-pattern graph query DSL.
///
/// Lets callers express depth-bounded traversal with edge-type and node
/// predicates without writing imperative BFS code.
///
/// # Example
/// ```ignore
/// let matches = store
///     .graph_query()
///     .from(&person_id)
///     .edge_types(["RELATES_TO", "KNOWS"])
///     .max_depth(3)
///     .filter_node(|obj| obj.object_id.starts_with("memory://"))
///     .execute()?;
/// ```
use crate::edge::BinderEdgeVersion;
use crate::engine::BinderStore;
use crate::error::StoreError;
use crate::object::BinderObjectVersion;
use crate::types::ObjectId;

type NodePredicate<'a> = Box<dyn Fn(&BinderObjectVersion) -> bool + 'a>;
type EdgePredicate<'a> = Box<dyn Fn(&BinderEdgeVersion) -> bool + 'a>;

// ── public types ──────────────────────────────────────────────────────────────

/// Direction of edge traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// Follow edges where the current node is the `from_object` (default).
    #[default]
    Outgoing,
    /// Follow edges where the current node is the `to_object`.
    Incoming,
    /// Follow edges in either direction.
    Both,
}

/// A single node returned by a graph query.
#[derive(Debug, Clone)]
pub struct GraphQueryMatch {
    /// The matched object at this position in the traversal.
    pub object: BinderObjectVersion,
    /// How many hops from the start node this object was found at.
    pub depth: usize,
    /// The edge that was traversed to reach this node (BFS first-arrival edge).
    /// `None` only when the match was synthesised outside of normal BFS (e.g.
    /// optional-clause results that carry no direct edge reference).
    pub via_edge: Option<BinderEdgeVersion>,
}

// ── builder ───────────────────────────────────────────────────────────────────

pub struct GraphQuery<'a> {
    pub(crate) store: &'a BinderStore,
    pub(crate) start: Option<ObjectId>,
    pub(crate) allowed_edge_types: Vec<String>,
    pub(crate) max_depth: usize,
    pub(crate) direction: Direction,
    pub(crate) node_filter: Option<NodePredicate<'a>>,
    pub(crate) edge_filter: Option<EdgePredicate<'a>>,
}

impl<'a> GraphQuery<'a> {
    pub(crate) fn new(store: &'a BinderStore) -> Self {
        Self {
            store,
            start: None,
            allowed_edge_types: Vec::new(),
            max_depth: 6,
            direction: Direction::Outgoing,
            node_filter: None,
            edge_filter: None,
        }
    }

    /// Set the start node.  Required before `execute()`.
    pub fn from(mut self, start: &ObjectId) -> Self {
        self.start = Some(start.clone());
        self
    }

    /// Restrict traversal to edges with one of the given relationship types.
    /// Calling this multiple times accumulates types (union).
    /// If never called, all edge types are followed.
    pub fn edge_types(mut self, types: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.allowed_edge_types
            .extend(types.into_iter().map(Into::into));
        self
    }

    /// Maximum number of hops from the start node (default: 6).
    pub fn max_depth(mut self, depth: usize) -> Self {
        self.max_depth = depth;
        self
    }

    /// Edge traversal direction (default: `Outgoing`).
    pub fn direction(mut self, d: Direction) -> Self {
        self.direction = d;
        self
    }

    /// Only include nodes in results where `predicate` returns `true`.
    /// Non-matching nodes are still traversed (path-inclusive semantics).
    pub fn filter_node(mut self, predicate: impl Fn(&BinderObjectVersion) -> bool + 'a) -> Self {
        self.node_filter = Some(Box::new(predicate));
        self
    }

    /// Only traverse edges where `predicate` returns `true`.
    /// Edges that fail this predicate are not followed.
    pub fn filter_edge(mut self, predicate: impl Fn(&BinderEdgeVersion) -> bool + 'a) -> Self {
        self.edge_filter = Some(Box::new(predicate));
        self
    }

    /// Execute the query and return all matching nodes.
    ///
    /// Results are in BFS order (ascending depth), but nodes at the same
    /// depth are in unspecified order.
    pub fn execute(self) -> Result<Vec<GraphQueryMatch>, StoreError> {
        self.store.execute_graph_query(self)
    }
}
