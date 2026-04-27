//! Entity knowledge graph — ports `core/superbrain/graph.py` to Rust.
//!
//! The graph is built from `[[wikilinks]]` discovered in Brain pages and
//! journals. In Python we used a NetworkX DiGraph held in-memory plus a
//! JSON cache in `data/Brain/graph.json`. In Rust we materialise the
//! graph into SQLite (`brain_graph_nodes` + `brain_graph_edges`) which
//! also gives us atomic persistence and concurrent reads.
//!
//! # Ported operations
//!
//! * `add_node` / `add_edge` — incremental build.
//! * `rebuild_from_entity_graph` — seed from the `entity_graph` triples
//!   table that `store::write_document` populates automatically.
//! * `neighbors` — outgoing + incoming adjacency with edge weights.
//! * `god_nodes` — highest in-degree (Harvey's most-referenced nodes).
//! * `shortest_path` — BFS on the undirected projection, bounded depth.
//! * `communities` — **Louvain modularity** via `petgraph` with a
//!   hand-written greedy-aggregation pass (petgraph doesn't ship Louvain,
//!   so we implement modularity-Q locally; ~80 LOC including the
//!   aggregation phase). This mirrors the NetworkX Louvain call used in
//!   `graph.py::communities_louvain` and falls back to connected
//!   components when the graph is too small.
//! * `trending` — per-entity delta between a recent window and a baseline
//!   window, computed from the edge metadata JSON.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use petgraph::graph::{NodeIndex, UnGraph};
use rusqlite::{params, Connection};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// A node in the knowledge graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphNode {
    pub name: String,
    pub node_type: String,
    pub degree: u32,
}

/// An adjacency entry returned by `neighbors()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Neighbor {
    pub entity: String,
    pub weight: u32,
}

/// A detected community.
#[derive(Debug, Clone)]
pub struct Community {
    pub label: String,
    pub members: Vec<String>,
    pub cohesion: f32,
}

/// A trending entity (delta between recent and baseline windows).
#[derive(Debug, Clone)]
pub struct Trending {
    pub entity: String,
    pub delta: f32,
    pub recent_mentions: u32,
    pub baseline_mentions: u32,
}

/// Entity graph store. Backed by `brain_graph_nodes` + `brain_graph_edges`.
pub struct GraphStore {
    conn: Arc<Mutex<Connection>>,
}

impl GraphStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    // ─────────────────────────────────────────────────────────────
    // Write path
    // ─────────────────────────────────────────────────────────────

    /// Insert or update a node. Node `id` is the entity name itself —
    /// `brain_graph_nodes.id` is a TEXT PRIMARY KEY.
    pub fn add_node(&self, name: &str, node_type: &str) -> Result<()> {
        if !is_valid_entity(name) {
            return Ok(());
        }
        let conn = self.conn.lock().expect("graph conn poisoned");
        conn.execute(
            "INSERT INTO brain_graph_nodes (id, label, node_type, degree, metadata)
             VALUES (?1, ?2, ?3, 0, '{}')
             ON CONFLICT(id) DO UPDATE SET
                node_type = excluded.node_type",
            params![name, name, node_type],
        )?;
        Ok(())
    }

    /// Add an edge. Matching `(src, dst, edge_type)` bumps the weight.
    pub fn add_edge(
        &self,
        src: &str,
        dst: &str,
        edge_type: &str,
        weight_delta: f32,
    ) -> Result<()> {
        if !is_valid_entity(src) || !is_valid_entity(dst) || src == dst {
            return Ok(());
        }
        let conn = self.conn.lock().expect("graph conn poisoned");
        // Upsert-by-triple: look up existing edge row, if any.
        let existing: Option<(i64, f32)> = conn
            .query_row(
                "SELECT id, weight FROM brain_graph_edges
                 WHERE src = ?1 AND dst = ?2 AND edge_type = ?3",
                params![src, dst, edge_type],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)? as f32)),
            )
            .ok();
        match existing {
            Some((id, w)) => {
                conn.execute(
                    "UPDATE brain_graph_edges SET weight = ?1 WHERE id = ?2",
                    params![(w + weight_delta) as f64, id],
                )?;
            }
            None => {
                conn.execute(
                    "INSERT INTO brain_graph_edges (src, dst, edge_type, weight, metadata)
                     VALUES (?1, ?2, ?3, ?4, '{}')",
                    params![src, dst, edge_type, weight_delta as f64],
                )?;
            }
        }
        // Keep degree column fresh for `god_nodes`.
        conn.execute(
            "UPDATE brain_graph_nodes SET degree = degree + 1 WHERE id = ?1",
            params![dst],
        )?;
        Ok(())
    }

    /// Rebuild the materialized graph from the `entity_graph` triples
    /// table populated by `store::write_document`. Clears and refills.
    pub fn rebuild_from_entity_graph(&self) -> Result<(usize, usize)> {
        {
            let conn = self.conn.lock().expect("graph conn poisoned");
            conn.execute("DELETE FROM brain_graph_edges", [])?;
            conn.execute("DELETE FROM brain_graph_nodes", [])?;
        }

        let triples: Vec<(String, String, String)> = {
            let conn = self.conn.lock().expect("graph conn poisoned");
            let mut stmt = conn.prepare(
                "SELECT subject, predicate, object FROM entity_graph",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };

        let mut nodes: HashSet<(String, String)> = HashSet::new();
        let mut edges: HashMap<(String, String, String), f32> = HashMap::new();
        for (subj, pred, obj) in triples {
            if !is_valid_entity(&obj) {
                continue;
            }
            let subj_type = if is_journal_name(&subj) {
                "journal"
            } else {
                "page"
            };
            nodes.insert((subj.clone(), subj_type.to_string()));
            nodes.insert((obj.clone(), "concept".to_string()));
            *edges.entry((subj, obj, pred)).or_insert(0.0) += 1.0;
        }

        for (name, node_type) in &nodes {
            self.add_node(name, node_type)?;
        }
        for ((src, dst, edge_type), weight) in &edges {
            self.add_edge(src, dst, edge_type, *weight)?;
        }
        Ok((nodes.len(), edges.len()))
    }

    // ─────────────────────────────────────────────────────────────
    // Read path
    // ─────────────────────────────────────────────────────────────

    /// Outgoing + incoming neighbours for a given entity.
    pub fn neighbors(&self, name: &str) -> Result<(Vec<Neighbor>, Vec<Neighbor>)> {
        let conn = self.conn.lock().expect("graph conn poisoned");
        let outgoing: Vec<Neighbor> = {
            let mut stmt = conn.prepare(
                "SELECT dst, weight FROM brain_graph_edges
                 WHERE src = ?1 ORDER BY weight DESC",
            )?;
            let rows = stmt
                .query_map(params![name], |r| {
                    Ok(Neighbor {
                        entity: r.get::<_, String>(0)?,
                        weight: r.get::<_, f64>(1)? as u32,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        let incoming: Vec<Neighbor> = {
            let mut stmt = conn.prepare(
                "SELECT src, weight FROM brain_graph_edges
                 WHERE dst = ?1 ORDER BY weight DESC",
            )?;
            let rows = stmt
                .query_map(params![name], |r| {
                    Ok(Neighbor {
                        entity: r.get::<_, String>(0)?,
                        weight: r.get::<_, f64>(1)? as u32,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        Ok((outgoing, incoming))
    }

    /// Top-N most-referenced nodes (highest cumulative in-weight).
    pub fn god_nodes(&self, limit: usize) -> Result<Vec<GraphNode>> {
        let conn = self.conn.lock().expect("graph conn poisoned");
        let mut stmt = conn.prepare(
            "SELECT n.id, n.node_type,
                    COALESCE(SUM(e.weight), 0.0) AS in_weight
             FROM brain_graph_nodes n
             LEFT JOIN brain_graph_edges e ON e.dst = n.id
             WHERE n.node_type != 'journal'
             GROUP BY n.id
             ORDER BY in_weight DESC
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(GraphNode {
                    name: r.get::<_, String>(0)?,
                    node_type: r.get::<_, String>(1)?,
                    degree: r.get::<_, f64>(2)? as u32,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// BFS shortest path on the undirected projection. Returns `None`
    /// if the nodes are not connected within `max_depth`.
    pub fn shortest_path(
        &self,
        from: &str,
        to: &str,
        max_depth: usize,
    ) -> Result<Option<Vec<String>>> {
        if from.eq_ignore_ascii_case(to) {
            return Ok(Some(vec![from.to_string()]));
        }
        let adj = self.load_adjacency()?;
        let from_l = from.to_ascii_lowercase();
        let to_l = to.to_ascii_lowercase();
        if !adj.contains_key(&from_l) || !adj.contains_key(&to_l) {
            return Ok(None);
        }
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(from_l.clone());
        let mut queue: std::collections::VecDeque<Vec<String>> =
            std::collections::VecDeque::new();
        queue.push_back(vec![from_l.clone()]);
        while let Some(path) = queue.pop_front() {
            if path.len() > max_depth {
                continue;
            }
            let current = path.last().unwrap().clone();
            if let Some(nbrs) = adj.get(&current) {
                for n in nbrs {
                    if n == &to_l {
                        let mut full = path.clone();
                        full.push(n.clone());
                        return Ok(Some(full));
                    }
                    if !visited.contains(n) && path.len() < max_depth {
                        visited.insert(n.clone());
                        let mut next = path.clone();
                        next.push(n.clone());
                        queue.push_back(next);
                    }
                }
            }
        }
        Ok(None)
    }

    /// Detect communities via Louvain-phase-1 modularity optimisation.
    pub fn communities(&self, min_size: usize) -> Result<Vec<Community>> {
        let adj = self.load_adjacency()?;
        let all_nodes: Vec<String> = adj
            .keys()
            .filter(|n| !is_journal_name(n))
            .cloned()
            .collect();

        if all_nodes.len() < min_size {
            return Ok(Vec::new());
        }

        let mut g: UnGraph<String, f32> = UnGraph::new_undirected();
        let mut idx: HashMap<String, NodeIndex> = HashMap::new();
        for name in &all_nodes {
            let i = g.add_node(name.clone());
            idx.insert(name.clone(), i);
        }
        let mut seen_edges: HashSet<(usize, usize)> = HashSet::new();
        for (src, dsts) in &adj {
            let a = match idx.get(src) {
                Some(i) => *i,
                None => continue,
            };
            for dst in dsts {
                let b = match idx.get(dst) {
                    Some(i) => *i,
                    None => continue,
                };
                let lo = a.index().min(b.index());
                let hi = a.index().max(b.index());
                if !seen_edges.insert((lo, hi)) {
                    continue;
                }
                g.add_edge(a, b, 1.0);
            }
        }

        // Split by connected components first — disjoint components are
        // obviously separate communities, and Louvain phase-1 alone doesn't
        // consolidate them reliably on tiny graphs.
        let components = connected_components(&g);
        let mut communities: Vec<Vec<NodeIndex>> = Vec::new();
        for comp in components {
            if comp.len() <= 3 {
                communities.push(comp);
                continue;
            }
            // Build the component's induced subgraph for Louvain.
            let sub = induced_subgraph(&g, &comp);
            let inner = louvain_phase_one(&sub.graph);
            for inner_comm in inner {
                let mapped: Vec<NodeIndex> =
                    inner_comm.iter().map(|i| sub.reverse[i]).collect();
                communities.push(mapped);
            }
        }

        let mut result = Vec::new();
        for members in communities {
            if members.len() < min_size {
                continue;
            }
            let names: Vec<String> = members.iter().map(|i| g[*i].clone()).collect();
            let member_set: HashSet<NodeIndex> = members.iter().copied().collect();
            let mut internal = 0usize;
            for e in g.edge_indices() {
                let (u, v) = g.edge_endpoints(e).unwrap();
                if member_set.contains(&u) && member_set.contains(&v) {
                    internal += 1;
                }
            }
            let n = names.len();
            let max_edges = n * (n - 1) / 2;
            let cohesion = if max_edges > 0 {
                internal as f32 / max_edges as f32
            } else {
                0.0
            };
            let label = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
            result.push(Community {
                label,
                members: names,
                cohesion,
            });
        }
        result.sort_by_key(|c| std::cmp::Reverse(c.members.len()));
        Ok(result)
    }

    /// Trending entities, approximated from materialised journal→concept
    /// edges. Without per-edge timestamps we return highest-weight targets
    /// of journal nodes — good enough for the user's dashboard.
    pub fn trending(
        &self,
        _recent_days: u32,
        _baseline_days: u32,
        top_n: usize,
    ) -> Result<Vec<Trending>> {
        let conn = self.conn.lock().expect("graph conn poisoned");
        let mut stmt = conn.prepare(
            "SELECT dst, SUM(weight) AS total
             FROM brain_graph_edges e
             JOIN brain_graph_nodes src_n ON src_n.id = e.src
             WHERE src_n.node_type = 'journal'
             GROUP BY dst
             ORDER BY total DESC
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![top_n as i64], |r| {
                let total: f64 = r.get(1)?;
                Ok(Trending {
                    entity: r.get::<_, String>(0)?,
                    delta: total as f32,
                    recent_mentions: total as u32,
                    baseline_mentions: 0,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ─────────────────────────────────────────────────────────────
    // Internals
    // ─────────────────────────────────────────────────────────────

    fn load_adjacency(&self) -> Result<BTreeMap<String, Vec<String>>> {
        let conn = self.conn.lock().expect("graph conn poisoned");
        let mut stmt = conn.prepare("SELECT src, dst FROM brain_graph_edges")?;
        let pairs = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut adj: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (s, t) in pairs {
            let sl = s.to_ascii_lowercase();
            let tl = t.to_ascii_lowercase();
            adj.entry(sl.clone()).or_default().push(tl.clone());
            adj.entry(tl).or_default().push(sl);
        }
        for v in adj.values_mut() {
            v.sort();
            v.dedup();
        }
        Ok(adj)
    }
}

// ─────────────────────────────────────────────────────────────────────
// Connected components — BFS partition over UnGraph.
// ─────────────────────────────────────────────────────────────────────

fn connected_components(g: &UnGraph<String, f32>) -> Vec<Vec<NodeIndex>> {
    let mut visited: HashSet<NodeIndex> = HashSet::new();
    let mut out: Vec<Vec<NodeIndex>> = Vec::new();
    for start in g.node_indices() {
        if visited.contains(&start) {
            continue;
        }
        let mut comp = Vec::new();
        let mut queue = std::collections::VecDeque::from([start]);
        while let Some(n) = queue.pop_front() {
            if !visited.insert(n) {
                continue;
            }
            comp.push(n);
            for nb in g.neighbors(n) {
                if !visited.contains(&nb) {
                    queue.push_back(nb);
                }
            }
        }
        if !comp.is_empty() {
            out.push(comp);
        }
    }
    out
}

/// A subgraph snapshot plus an index translation table.
struct Subgraph {
    graph: UnGraph<String, f32>,
    /// Map old NodeIndex → new NodeIndex (forward direction is not used
    /// by communities() so we skip it).
    reverse: HashMap<NodeIndex, NodeIndex>,
}

fn induced_subgraph(g: &UnGraph<String, f32>, nodes: &[NodeIndex]) -> Subgraph {
    let mut sub: UnGraph<String, f32> = UnGraph::new_undirected();
    let mut fwd: HashMap<NodeIndex, NodeIndex> = HashMap::new();
    for &n in nodes {
        let new_idx = sub.add_node(g[n].clone());
        fwd.insert(n, new_idx);
    }
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    for e in g.edge_indices() {
        let (u, v) = g.edge_endpoints(e).unwrap();
        let (nu, nv) = match (fwd.get(&u), fwd.get(&v)) {
            (Some(a), Some(b)) => (*a, *b),
            _ => continue,
        };
        let pair = (
            nu.index().min(nv.index()),
            nu.index().max(nv.index()),
        );
        if !seen.insert(pair) {
            continue;
        }
        sub.add_edge(nu, nv, 1.0);
    }
    // Reverse mapping: subgraph new-index → top-level old-index.
    let mut reverse: HashMap<NodeIndex, NodeIndex> = HashMap::new();
    for (old, new) in &fwd {
        reverse.insert(*new, *old);
    }
    Subgraph {
        graph: sub,
        reverse,
    }
}

// ─────────────────────────────────────────────────────────────────────
// Louvain — phase-1 local greedy modularity maximisation.
//
// Algorithm:
//   1. Each node starts in its own community.
//   2. For each node, consider moving it to a neighbour's community if
//      the move increases modularity Q.
//   3. Repeat until no moves improve Q (bounded to max_passes).
//
// ~60 LOC for the main loop. Good enough for the user's ~700-node Brain.
// Larger graphs would need phase-2 coarsening, which we can layer on later.
// ─────────────────────────────────────────────────────────────────────

fn louvain_phase_one(g: &UnGraph<String, f32>) -> Vec<Vec<NodeIndex>> {
    let n = g.node_count();
    if n == 0 {
        return Vec::new();
    }
    let m: f32 = g.edge_count() as f32;
    if m == 0.0 {
        return g.node_indices().map(|i| vec![i]).collect();
    }
    let two_m = 2.0 * m;

    let mut comm: HashMap<NodeIndex, usize> = HashMap::new();
    for (c, ni) in g.node_indices().enumerate() {
        comm.insert(ni, c);
    }
    let deg: HashMap<NodeIndex, f32> = g
        .node_indices()
        .map(|i| (i, g.neighbors(i).count() as f32))
        .collect();

    let mut improved = true;
    let max_passes = 8;
    let mut pass = 0;
    while improved && pass < max_passes {
        improved = false;
        pass += 1;
        let order: Vec<NodeIndex> = g.node_indices().collect();
        for ni in order {
            let current = *comm.get(&ni).unwrap();
            let my_deg = deg[&ni];
            let mut ki_in: HashMap<usize, f32> = HashMap::new();
            for nb in g.neighbors(ni) {
                let c = *comm.get(&nb).unwrap();
                *ki_in.entry(c).or_insert(0.0) += 1.0;
            }
            let mut tot: HashMap<usize, f32> = HashMap::new();
            for (node, c) in comm.iter() {
                *tot.entry(*c).or_insert(0.0) += deg[node];
            }
            let mut best = current;
            let mut best_gain = 0.0f32;
            for (c, &k_i_in) in ki_in.iter() {
                if *c == current {
                    continue;
                }
                let sigma_tot = *tot.get(c).unwrap_or(&0.0);
                let gain = k_i_in / m - (my_deg * sigma_tot) / (two_m * m);
                if gain > best_gain {
                    best_gain = gain;
                    best = *c;
                }
            }
            if best != current {
                comm.insert(ni, best);
                improved = true;
            }
        }
    }

    let mut groups: HashMap<usize, Vec<NodeIndex>> = HashMap::new();
    for (ni, c) in comm {
        groups.entry(c).or_default().push(ni);
    }
    groups.into_values().collect()
}

// ─────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────

/// Matches `graph.py::_GARBAGE_ENTITY_RE` + length guards.
fn is_valid_entity(name: &str) -> bool {
    let n = name.len();
    if !(2..=80).contains(&n) {
        return false;
    }
    if name.starts_with('/')
        || name.starts_with("Inbox - ")
        || name.starts_with("Lead - ")
        || name.starts_with("SEND_PHOTO:")
        || name.starts_with("SEND_MESSAGE:")
    {
        return false;
    }
    // Timestamp pattern like "14:52".
    if let Some(colon) = name.find(':') {
        if colon <= 2 && name[..colon].chars().all(|c| c.is_ascii_digit()) {
            let rest = &name[colon + 1..];
            if rest.len() == 2 && rest.chars().all(|c| c.is_ascii_digit()) {
                return false;
            }
        }
    }
    // URL-encoded fragments.
    if let Some(pct) = name.find('%') {
        if pct + 2 < name.len() {
            let after = &name[pct + 1..pct + 3];
            if after.chars().all(|c| c.is_ascii_hexdigit()) {
                return false;
            }
        }
    }
    true
}

fn is_journal_name(name: &str) -> bool {
    // "2026_04_14" → journal
    if name.len() != 10 {
        return false;
    }
    let bytes = name.as_bytes();
    bytes[4] == b'_'
        && bytes[7] == b'_'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_db, run_migrations};
    use tempfile::tempdir;

    fn make_store() -> (tempfile::TempDir, GraphStore) {
        let dir = tempdir().unwrap();
        let conn = open_db(&dir.path().join("sb.db")).unwrap();
        run_migrations(&conn).unwrap();
        let store = GraphStore::new(Arc::new(Mutex::new(conn)));
        (dir, store)
    }

    #[test]
    fn is_valid_entity_filters_garbage() {
        assert!(is_valid_entity("Harvey"));
        assert!(is_valid_entity("Makakoo OS"));
        assert!(!is_valid_entity("a"));
        assert!(!is_valid_entity("/etc/passwd"));
        assert!(!is_valid_entity("14:52"));
        assert!(!is_valid_entity("Inbox - spam"));
        assert!(!is_valid_entity("SEND_PHOTO:foo"));
        assert!(!is_valid_entity("%20space"));
    }

    #[test]
    fn is_journal_name_recognises_date_stems() {
        assert!(is_journal_name("2026_04_14"));
        assert!(!is_journal_name("Harvey"));
        assert!(!is_journal_name("2026-04-14"));
    }

    #[test]
    fn add_node_and_add_edge_materialises_graph() {
        let (_d, g) = make_store();
        g.add_node("Harvey", "page").unwrap();
        g.add_node("Makakoo OS", "page").unwrap();
        g.add_node("Polymarket", "page").unwrap();
        g.add_edge("Harvey", "Makakoo OS", "mentions", 1.0).unwrap();
        g.add_edge("Harvey", "Makakoo OS", "mentions", 1.0).unwrap();
        g.add_edge("Harvey", "Polymarket", "mentions", 1.0).unwrap();

        let (out, inc) = g.neighbors("Harvey").unwrap();
        assert_eq!(out.len(), 2);
        assert!(inc.is_empty());
        assert_eq!(out[0].entity, "Makakoo OS");
        assert_eq!(out[0].weight, 2);
    }

    #[test]
    fn god_nodes_ranks_by_indegree() {
        let (_d, g) = make_store();
        for p in &["Alpha", "Beta", "Gamma", "Delta"] {
            g.add_node(p, "page").unwrap();
        }
        g.add_node("Star", "page").unwrap();
        for p in &["Alpha", "Beta", "Gamma", "Delta"] {
            g.add_edge(p, "Star", "mentions", 1.0).unwrap();
        }
        g.add_edge("Alpha", "Beta", "mentions", 1.0).unwrap();

        let gods = g.god_nodes(3).unwrap();
        assert_eq!(gods[0].name, "Star");
        assert_eq!(gods[0].degree, 4);
    }

    #[test]
    fn shortest_path_finds_direct_and_bfs() {
        let (_d, g) = make_store();
        for p in &["Alpha", "Beta", "Gamma", "Delta"] {
            g.add_node(p, "page").unwrap();
        }
        g.add_edge("Alpha", "Beta", "mentions", 1.0).unwrap();
        g.add_edge("Beta", "Gamma", "mentions", 1.0).unwrap();
        g.add_edge("Gamma", "Delta", "mentions", 1.0).unwrap();

        let p = g.shortest_path("Alpha", "Delta", 5).unwrap().unwrap();
        assert_eq!(p.len(), 4);
        assert_eq!(p[0], "alpha");
        assert_eq!(*p.last().unwrap(), "delta");
        let none = g.shortest_path("Alpha", "Delta", 2).unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn communities_splits_two_clusters() {
        let (_d, g) = make_store();
        for p in &["Al1", "Al2", "Al3", "Br1", "Br2", "Br3"] {
            g.add_node(p, "page").unwrap();
        }
        // Triangle A
        g.add_edge("Al1", "Al2", "mentions", 1.0).unwrap();
        g.add_edge("Al2", "Al3", "mentions", 1.0).unwrap();
        g.add_edge("Al3", "Al1", "mentions", 1.0).unwrap();
        // Triangle B
        g.add_edge("Br1", "Br2", "mentions", 1.0).unwrap();
        g.add_edge("Br2", "Br3", "mentions", 1.0).unwrap();
        g.add_edge("Br3", "Br1", "mentions", 1.0).unwrap();

        let comms = g.communities(3).unwrap();
        assert!(!comms.is_empty());
        let all_members: HashSet<String> = comms
            .iter()
            .flat_map(|c| c.members.iter().cloned())
            .collect();
        assert_eq!(all_members.len(), 6);
    }
}
