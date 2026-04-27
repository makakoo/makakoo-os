#!/usr/bin/env python3
"""
Brain Knowledge Graph — Entity relationship graph built from Brain wikilinks.

Parses [[wikilinks]] from all Brain pages and journals, builds a NetworkX
directed graph, and exposes structural analysis: god nodes, communities,
path finding, and neighbor discovery.

Usage:
    python3 graph.py --build              # build/rebuild graph from Brain
    python3 graph.py --god-nodes          # most referenced entities
    python3 graph.py --communities        # thematic knowledge clusters
    python3 graph.py --neighbors "Traylinx"
    python3 graph.py --path "Harvey OS" "Polymarket"
    python3 graph.py --stats              # graph summary
"""

import json
import logging
import os
import re
import sys
from collections import Counter, defaultdict
from datetime import datetime, timedelta
from pathlib import Path
from typing import Dict, List, Optional, Set, Tuple

try:
    import networkx as nx
    HAS_NETWORKX = True
except ImportError:
    HAS_NETWORKX = False

sys.path.insert(0, str(Path(__file__).resolve().parent.parent.parent))

from core.superbrain import config

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(name)s] %(levelname)s: %(message)s",
    datefmt="%H:%M:%S",
)
log = logging.getLogger("superbrain.graph")

GRAPH_PATH = os.path.join(config.BRAIN_DIR, "graph.json")
WIKILINK_RE = re.compile(r"\[\[([^\]]+)\]\]")

# Entities matching these patterns are noise, not knowledge
_GARBAGE_ENTITY_RE = re.compile(
    r"^/|"                           # file paths
    r"^\d{1,2}:\d{2}$|"             # timestamps like 14:52
    r"^Inbox - |"                    # email inbox entries
    r"^Lead - |"                     # CRM lead dumps
    r"%[0-9A-Fa-f]{2}|"             # URL-encoded strings
    r"^SEND_PHOTO:|^SEND_MESSAGE:"   # Telegram error artifacts
)


def _is_valid_entity(name: str) -> bool:
    """Filter out garbage entities that aren't real knowledge nodes."""
    if len(name) < 2 or len(name) > 80:
        return False
    if _GARBAGE_ENTITY_RE.search(name):
        return False
    return True


# ─────────────────────────────────────────────────────────────────────────────
#  Graph Build
# ─────────────────────────────────────────────────────────────────────────────

def _extract_wikilinks(text: str) -> List[str]:
    """Extract all [[wikilink]] targets from markdown text."""
    return [m.group(1).strip() for m in WIKILINK_RE.finditer(text)]


def _page_name_from_path(path: Path) -> str:
    """Convert file path to page name (strip .md, decode URL encoding)."""
    name = path.stem
    # Brain files encode some chars — decode basic cases
    name = name.replace("%2F", "/").replace("%20", " ")
    return name


def build_graph(brain_dir: str = None) -> dict:
    """
    Parse Brain pages and journals, extract [[wikilinks]], build entity graph.

    Returns graph as a dict with nodes/edges/metadata for serialization.
    """
    brain_dir = brain_dir or config.BRAIN_DIR
    pages_dir = Path(brain_dir) / "pages"
    journals_dir = Path(brain_dir) / "journals"

    # Node: entity name → {type, mention_count, mentioned_from: [source_names]}
    nodes: Dict[str, dict] = {}
    # Edge: (source, target) → {weight, sources: [file names]}
    edges: Dict[Tuple[str, str], dict] = {}

    def _register_node(name: str, node_type: str):
        if name not in nodes:
            nodes[name] = {"type": node_type, "mention_count": 0, "mentioned_from": []}

    def _add_edge(source: str, target: str, source_file: str):
        key = (source, target)
        if key not in edges:
            edges[key] = {"weight": 0, "sources": []}
        edges[key]["weight"] += 1
        if source_file not in edges[key]["sources"]:
            edges[key]["sources"].append(source_file)
        nodes[target]["mention_count"] += 1
        if source_file not in nodes[target]["mentioned_from"]:
            nodes[target]["mentioned_from"].append(source_file)

    # Process pages
    page_files = list(pages_dir.glob("*.md")) if pages_dir.exists() else []
    for path in page_files:
        page_name = _page_name_from_path(path)
        _register_node(page_name, "page")
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except Exception:
            continue
        for link in _extract_wikilinks(text):
            _register_node(link, "page")
            if link != page_name:
                _add_edge(page_name, link, path.name)

    # Process journals
    journal_files = list(journals_dir.glob("*.md")) if journals_dir.exists() else []
    for path in journal_files:
        journal_date = path.stem
        _register_node(journal_date, "journal")
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except Exception:
            continue
        for link in _extract_wikilinks(text):
            _register_node(link, "concept")
            _add_edge(journal_date, link, path.name)

    graph = {
        "nodes": {k: v for k, v in nodes.items()},
        "edges": [
            {
                "source": s,
                "target": t,
                "weight": data["weight"],
                "sources": data["sources"],
            }
            for (s, t), data in edges.items()
        ],
        "meta": {
            "pages": len(page_files),
            "journals": len(journal_files),
            "total_nodes": len(nodes),
            "total_edges": len(edges),
        },
    }
    log.info(
        "Built graph: %d nodes, %d edges from %d pages + %d journals",
        len(nodes), len(edges), len(page_files), len(journal_files),
    )
    return graph


def save_graph(graph: dict, path: str = GRAPH_PATH):
    """Persist graph to JSON."""
    with open(path, "w", encoding="utf-8") as f:
        json.dump(graph, f, indent=2, ensure_ascii=False)
    log.info("Graph saved → %s", path)


def load_graph(path: str = GRAPH_PATH) -> Optional[dict]:
    """Load persisted graph or return None if not built yet."""
    if not os.path.exists(path):
        return None
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def get_or_build_graph(force: bool = False) -> dict:
    """Load graph if fresh; otherwise build and save."""
    if not force:
        graph = load_graph()
        if graph:
            return graph
    graph = build_graph()
    save_graph(graph)
    return graph


# ─────────────────────────────────────────────────────────────────────────────
#  Analysis
# ─────────────────────────────────────────────────────────────────────────────

def god_nodes(graph: dict, top_n: int = 15, exclude_journals: bool = True) -> List[dict]:
    """
    Return the most-referenced entities (highest in-degree / mention_count).
    These are Harvey's most central knowledge nodes.
    """
    nodes = graph["nodes"]
    ranked = []
    for name, data in nodes.items():
        if exclude_journals and data.get("type") == "journal":
            continue
        ranked.append({
            "name": name,
            "mentions": data["mention_count"],
            "type": data["type"],
            "mentioned_from": data["mentioned_from"][:5],  # sample sources
        })
    ranked.sort(key=lambda x: x["mentions"], reverse=True)
    return ranked[:top_n]


def neighbors(graph: dict, entity: str, direction: str = "both") -> dict:
    """
    Return entities directly connected to the given node.
    direction: "outgoing" (what entity links to), "incoming" (who links to entity), "both"
    """
    entity_lower = entity.lower()
    outgoing = []
    incoming = []

    for edge in graph["edges"]:
        s, t = edge["source"], edge["target"]
        if direction in ("outgoing", "both") and s.lower() == entity_lower:
            outgoing.append({"entity": t, "weight": edge["weight"], "sources": edge["sources"][:3]})
        if direction in ("incoming", "both") and t.lower() == entity_lower:
            incoming.append({"entity": s, "weight": edge["weight"], "sources": edge["sources"][:3]})

    outgoing.sort(key=lambda x: x["weight"], reverse=True)
    incoming.sort(key=lambda x: x["weight"], reverse=True)
    return {"outgoing": outgoing, "incoming": incoming}


def find_path(graph: dict, source: str, target: str, max_depth: int = 4) -> Optional[List[str]]:
    """
    BFS shortest path between two entities in the graph.
    Returns node list or None if not connected within max_depth.
    """
    source_l = source.lower()
    target_l = target.lower()

    # Build adjacency (case-insensitive)
    adj: Dict[str, Set[str]] = defaultdict(set)
    name_map: Dict[str, str] = {}

    for node_name in graph["nodes"]:
        name_map[node_name.lower()] = node_name

    for edge in graph["edges"]:
        s_l = edge["source"].lower()
        t_l = edge["target"].lower()
        adj[s_l].add(t_l)
        adj[t_l].add(s_l)  # undirected for path finding

    if source_l not in name_map or target_l not in name_map:
        return None

    # BFS
    visited = {source_l}
    queue = [[source_l]]
    while queue:
        path = queue.pop(0)
        if len(path) > max_depth:
            return None
        current = path[-1]
        for neighbor in adj[current]:
            if neighbor == target_l:
                full_path = path + [neighbor]
                return [name_map.get(n, n) for n in full_path]
            if neighbor not in visited:
                visited.add(neighbor)
                queue.append(path + [neighbor])
    return None


def communities(graph: dict, min_size: int = 3) -> List[dict]:
    """
    Detect knowledge communities using simple connected components + density analysis.
    Falls back to Leiden if graspologic is available.
    Returns list of {label, members, cohesion_score} dicts.
    """
    # Build adjacency for non-journal nodes with weight > 0
    adj: Dict[str, Set[str]] = defaultdict(set)
    all_nodes: Set[str] = set()

    for edge in graph["edges"]:
        s, t = edge["source"], edge["target"]
        s_type = graph["nodes"].get(s, {}).get("type", "")
        t_type = graph["nodes"].get(t, {}).get("type", "")
        # Skip journal-to-concept edges for community detection (too noisy)
        if s_type == "journal":
            continue
        if edge["weight"] > 0:
            adj[s].add(t)
            adj[t].add(s)
            all_nodes.add(s)
            all_nodes.add(t)

    # Connected components via BFS
    visited: Set[str] = set()
    components = []

    for start in all_nodes:
        if start in visited:
            continue
        component = set()
        queue = [start]
        while queue:
            node = queue.pop()
            if node in visited:
                continue
            visited.add(node)
            component.add(node)
            queue.extend(adj[node] - visited)
        components.append(component)

    # Filter and score
    result = []
    for i, component in enumerate(sorted(components, key=len, reverse=True)):
        if len(component) < min_size:
            continue
        # Cohesion = edges within / total possible edges
        members = list(component)
        internal_edges = sum(
            1 for e in graph["edges"]
            if e["source"] in component and e["target"] in component
        )
        max_edges = len(members) * (len(members) - 1)
        cohesion = internal_edges / max_edges if max_edges > 0 else 0

        # Label by most-cited member
        top = max(
            members,
            key=lambda n: graph["nodes"].get(n, {}).get("mention_count", 0),
        )
        result.append({
            "label": f"Community: {top}",
            "top_node": top,
            "size": len(members),
            "members": sorted(members, key=lambda n: graph["nodes"].get(n, {}).get("mention_count", 0), reverse=True)[:10],
            "cohesion_score": round(cohesion, 3),
            "internal_edges": internal_edges,
        })

    return result[:20]


def graph_context_for_query(graph: dict, query: str, top_n: int = 5) -> str:
    """
    Given a free-text query, find relevant graph entities and return
    a summary of their relationships. Used to augment superbrain queries.

    Uses PPR when available for relevance-ranked entity discovery,
    falling back to keyword matching if NetworkX is unavailable.
    """
    # Try PPR-ranked entities first (much better than keyword matching)
    ppr_scores = personalized_pagerank(graph, query) if HAS_NETWORKX else {}

    if ppr_scores:
        # Use PPR-ranked entities
        ranked = sorted(ppr_scores.items(), key=lambda x: x[1], reverse=True)
        top_matches = [(name, score) for name, score in ranked[:top_n]]
    else:
        # Fallback: keyword matching
        query_lower = query.lower()
        query_terms = set(query_lower.split())
        matches = []
        for name, data in graph["nodes"].items():
            if data.get("type") == "journal":
                continue
            name_lower = name.lower()
            name_terms = set(name_lower.split())
            overlap = len(query_terms & name_terms)
            if query_lower in name_lower or overlap > 0:
                matches.append((name, data["mention_count"]))
        matches.sort(key=lambda x: x[1], reverse=True)
        top_matches = matches[:top_n]

    if not top_matches:
        return ""

    parts = []
    top_names = {name for name, _ in top_matches}
    for name, _ in top_matches:
        nb = neighbors(graph, name)
        out_names = [e["entity"] for e in nb["outgoing"][:5]]
        in_names = [e["entity"] for e in nb["incoming"][:5] if e["entity"] not in top_names]
        mc = graph["nodes"].get(name, {}).get("mention_count", 0)
        parts.append(
            f"**{name}** ({mc}x)"
            + (f" → {', '.join(out_names)}" if out_names else "")
            + (f" ← {', '.join(in_names[:3])}" if in_names else "")
        )

    return "Graph: " + " | ".join(parts)


# ─────────────────────────────────────────────────────────────────────────────
#  SQLite → NetworkX (single source of truth)
# ─────────────────────────────────────────────────────────────────────────────

def load_from_store(store) -> dict:
    """Build graph dict from store's entity_graph SQLite table (source of truth).

    This replaces filesystem parsing for all analysis operations.
    store must have a _conn attribute (sqlite3 connection with row_factory).
    """
    rows = store._conn.execute(
        "SELECT subject, predicate, object, valid_from, confidence, source "
        "FROM entity_graph"
    ).fetchall()

    nodes: Dict[str, dict] = {}
    edges: Dict[Tuple[str, str], dict] = {}

    def _ensure_node(name: str, node_type: str = "concept"):
        if name not in nodes:
            nodes[name] = {"type": node_type, "mention_count": 0, "mentioned_from": []}

    for r in rows:
        subj, obj = r["subject"], r["object"]
        # Skip garbage entities
        if not _is_valid_entity(obj):
            continue
        # Detect type from naming patterns
        subj_type = "journal" if re.match(r"^\d{4}_\d{2}_\d{2}$", subj) else "page"
        _ensure_node(subj, subj_type)
        _ensure_node(obj, "concept")

        key = (subj, obj)
        if key not in edges:
            edges[key] = {"weight": 0, "sources": [], "valid_from": []}
        edges[key]["weight"] += 1
        src = r["source"] or ""
        if src and src not in edges[key]["sources"]:
            edges[key]["sources"].append(src)
        if r["valid_from"]:
            edges[key]["valid_from"].append(r["valid_from"])

        nodes[obj]["mention_count"] += 1
        if subj not in nodes[obj]["mentioned_from"]:
            nodes[obj]["mentioned_from"].append(subj)

    # Count pages and journals from node types
    n_journals = sum(1 for n in nodes.values() if n["type"] == "journal")
    n_pages = len(nodes) - n_journals

    graph = {
        "nodes": nodes,
        "edges": [
            {"source": s, "target": t, "weight": d["weight"],
             "sources": d["sources"], "valid_from": d.get("valid_from", [])}
            for (s, t), d in edges.items()
        ],
        "meta": {
            "pages": n_pages, "journals": n_journals,
            "total_nodes": len(nodes), "total_edges": len(edges),
        },
    }
    log.info("Loaded graph from store: %d nodes, %d edges", len(nodes), len(edges))
    return graph


def to_networkx(graph: dict) -> "nx.DiGraph":
    """Convert graph dict to NetworkX DiGraph. Requires networkx."""
    if not HAS_NETWORKX:
        raise ImportError("networkx required for graph analysis. pip install networkx")

    G = nx.DiGraph()
    for name, data in graph["nodes"].items():
        G.add_node(name, type=data.get("type", "concept"),
                   mention_count=data.get("mention_count", 0))

    for edge in graph["edges"]:
        G.add_edge(edge["source"], edge["target"],
                   weight=edge["weight"],
                   valid_from=edge.get("valid_from", []))
    return G


# ─────────────────────────────────────────────────────────────────────────────
#  Louvain Community Detection (replaces BFS connected components)
# ─────────────────────────────────────────────────────────────────────────────

def communities_louvain(graph: dict, resolution: float = 1.0,
                        min_size: int = 3, max_display: int = 50) -> List[dict]:
    """Detect knowledge communities using Louvain modularity optimization.

    Falls back to greedy modularity, then to BFS connected components.
    """
    if not HAS_NETWORKX:
        return communities(graph, min_size=min_size)  # existing BFS fallback

    G = to_networkx(graph)

    # Filter: remove journal nodes AND hub index pages (too noisy)
    # Hub threshold: nodes with >100 outgoing edges are index pages, not knowledge
    HUB_THRESHOLD = 100
    noisy_nodes = [
        n for n, d in G.nodes(data=True)
        if d.get("type") == "journal" or G.out_degree(n) > HUB_THRESHOLD
    ]
    G_filtered = G.copy()
    G_filtered.remove_nodes_from(noisy_nodes)

    # Convert to undirected for community detection
    G_undirected = G_filtered.to_undirected()

    # Remove isolated nodes
    isolates = list(nx.isolates(G_undirected))
    G_undirected.remove_nodes_from(isolates)

    if len(G_undirected) < min_size:
        return []

    # Try Louvain first, then greedy modularity
    try:
        comms = nx.community.louvain_communities(
            G_undirected, resolution=resolution, seed=42
        )
    except Exception:
        try:
            comms = list(nx.community.greedy_modularity_communities(G_undirected))
        except Exception:
            return communities(graph, min_size=min_size)  # BFS fallback

    result = []
    for community_set in sorted(comms, key=len, reverse=True):
        if len(community_set) < min_size:
            continue

        members = list(community_set)
        # Cohesion: internal edges / total possible
        subG = G_undirected.subgraph(members)
        internal_edges = subG.number_of_edges()
        max_edges = len(members) * (len(members) - 1) // 2
        cohesion = internal_edges / max_edges if max_edges > 0 else 0

        # Label by top 3 members (highest mention count)
        ranked = sorted(
            members,
            key=lambda n: graph["nodes"].get(n, {}).get("mention_count", 0),
            reverse=True,
        )
        top3 = ranked[:3]
        label = ", ".join(top3)

        result.append({
            "label": label,
            "top_nodes": top3,
            "size": len(members),
            "members": ranked[:max_display],
            "cohesion_score": round(cohesion, 3),
            "internal_edges": internal_edges,
        })

    return result[:20]


# ─────────────────────────────────────────────────────────────────────────────
#  Personalized PageRank
# ─────────────────────────────────────────────────────────────────────────────

# Cache for PPR results: (graph_gen, query_key) → normalized scores dict
_ppr_cache: Dict[str, Dict[str, float]] = {}
_ppr_cache_gen: int = 0  # bumped on graph rebuild to invalidate stale entries
_PPR_CACHE_MAX = 100


def invalidate_ppr_cache():
    """Clear PPR cache and bump generation. Called after graph rebuild."""
    global _ppr_cache_gen
    _ppr_cache.clear()
    _ppr_cache_gen += 1


def personalized_pagerank(graph: dict, query: str,
                          min_word_len: int = 4) -> Dict[str, float]:
    """Compute Personalized PageRank seeded on query-matching entities.

    Returns dict of {entity_name: normalized_score} where scores are in [0, 1].
    Cache is generation-aware: invalidated when graph is rebuilt.
    """
    if not HAS_NETWORKX:
        return {}

    # Cache key includes graph generation to avoid stale results
    query_words = set(w.lower() for w in query.split() if len(w) >= min_word_len)
    if not query_words:
        return {}
    cache_key = f"{_ppr_cache_gen}|" + "|".join(sorted(query_words))

    if cache_key in _ppr_cache:
        return _ppr_cache[cache_key]

    G = to_networkx(graph)
    if G.number_of_nodes() == 0:
        return {}

    # Find seed entities matching query words
    seeds = set()
    for node in G.nodes:
        node_lower = node.lower()
        if any(w in node_lower for w in query_words):
            seeds.add(node)

    if not seeds:
        return {}

    # Build personalization vector: seed nodes get equal weight
    personalization = {n: 0.0 for n in G.nodes}
    seed_weight = 1.0 / len(seeds)
    for s in seeds:
        personalization[s] = seed_weight

    # Compute PPR
    try:
        raw_scores = nx.pagerank(G, alpha=0.85, personalization=personalization,
                                 max_iter=100, tol=1e-06)
    except Exception:
        return {}

    # Normalize to [0, 1] — divide by max score
    max_score = max(raw_scores.values()) if raw_scores else 1.0
    if max_score == 0:
        return {}
    normalized = {k: v / max_score for k, v in raw_scores.items()}

    # Filter out journal nodes and low scores
    result = {k: v for k, v in normalized.items()
              if v > 0.01 and graph["nodes"].get(k, {}).get("type") != "journal"}

    # Cache (LRU eviction)
    if len(_ppr_cache) >= _PPR_CACHE_MAX:
        # Remove oldest entry
        oldest = next(iter(_ppr_cache))
        del _ppr_cache[oldest]
    _ppr_cache[cache_key] = result

    return result


def ppr_boost_hits(graph: dict, hits: list, question: str,
                   weight: float = 0.3) -> list:
    """Apply PPR-based boosting to search hits.

    Replaces the raw SQL _graph_boost() in superbrain.py.
    Uses normalized PPR scores (0-1) × weight for multiplicative boost.

    Direct entity match: score *= (1 + ppr_score × weight × 2)  [stronger]
    Neighbor match:      score *= (1 + ppr_score × weight)       [weaker]
    """
    ppr_scores = personalized_pagerank(graph, question)
    if not ppr_scores:
        return hits

    for h in hits:
        title_lower = h.title.lower()

        # Check for direct entity match
        best_direct = 0.0
        best_neighbor = 0.0

        for entity, score in ppr_scores.items():
            entity_lower = entity.lower()
            if entity_lower in title_lower or title_lower in entity_lower:
                best_direct = max(best_direct, score)

        # If no direct match, check if title matches any entity's name loosely
        if best_direct == 0.0:
            # Check if any high-PPR entity is a neighbor concept
            for entity, score in ppr_scores.items():
                if score > 0.1:  # only consider significant entities
                    entity_lower = entity.lower()
                    # Check word overlap
                    entity_words = set(entity_lower.split())
                    title_words = set(title_lower.replace("_", " ").split())
                    if entity_words & title_words:
                        best_neighbor = max(best_neighbor, score)

        if best_direct > 0:
            h.score *= (1 + best_direct * weight * 2)
        elif best_neighbor > 0:
            h.score *= (1 + best_neighbor * weight)

    return hits


# ─────────────────────────────────────────────────────────────────────────────
#  Temporal Trending
# ─────────────────────────────────────────────────────────────────────────────

def trending_entities(graph: dict, days_recent: int = 7,
                      days_baseline: int = 30, top_n: int = 15) -> List[dict]:
    """Find entities trending up or down in recent vs baseline period.

    Uses valid_from dates from journal edges to compute time-windowed centrality.
    """
    now = datetime.now()
    recent_cutoff = (now - timedelta(days=days_recent)).strftime("%Y-%m-%d")
    baseline_cutoff = (now - timedelta(days=days_baseline)).strftime("%Y-%m-%d")

    recent_counts: Counter = Counter()
    baseline_counts: Counter = Counter()

    for edge in graph["edges"]:
        target = edge["target"]
        for vf in edge.get("valid_from", []):
            if not vf:
                continue
            if vf >= recent_cutoff:
                recent_counts[target] += edge["weight"]
            if vf >= baseline_cutoff:
                baseline_counts[target] += edge["weight"]

    # Normalize: per-day rates
    results = []
    for entity in set(recent_counts) | set(baseline_counts):
        recent_rate = recent_counts.get(entity, 0) / max(days_recent, 1)
        baseline_rate = baseline_counts.get(entity, 0) / max(days_baseline, 1)
        delta = recent_rate - baseline_rate

        if abs(delta) < 0.01:
            continue  # noise filter

        node_data = graph["nodes"].get(entity, {})
        if node_data.get("type") == "journal":
            continue  # skip journal nodes

        results.append({
            "entity": entity,
            "delta": round(delta, 3),
            "recent_mentions": recent_counts.get(entity, 0),
            "baseline_mentions": baseline_counts.get(entity, 0),
            "direction": "up" if delta > 0 else "down",
        })

    results.sort(key=lambda x: abs(x["delta"]), reverse=True)
    return results[:top_n]


# ─────────────────────────────────────────────────────────────────────────────
#  Knowledge Map (dashboard)
# ─────────────────────────────────────────────────────────────────────────────

def knowledge_map(graph: dict, resolution: float = 1.0) -> str:
    """Generate a text dashboard: communities + god nodes + trending."""
    lines = []
    lines.append("")
    lines.append("Harvey Brain — Knowledge Map")
    lines.append("=" * 50)

    # Communities
    comms = communities_louvain(graph, resolution=resolution)
    lines.append(f"\nCommunities ({len(comms)} detected):")
    for c in comms[:10]:
        top = ", ".join(c["top_nodes"][:3])
        lines.append(f"  [{top}]  {c['size']} nodes, cohesion={c['cohesion_score']}")

    # God nodes
    gods = god_nodes(graph, top_n=10)
    lines.append(f"\nGod Nodes (top {len(gods)}):")
    pairs = []
    for g in gods:
        pairs.append(f"  {g['name']:<25} {g['mentions']:>3}x")
    lines.extend(pairs)

    # Trending
    trends = trending_entities(graph)
    if trends:
        lines.append(f"\nTrending (7-day delta):")
        for t in trends[:8]:
            arrow = "↑" if t["direction"] == "up" else "↓"
            lines.append(f"  {arrow} {t['entity']:<25} {t['delta']:+.2f}  "
                         f"(7d:{t['recent_mentions']}, 30d:{t['baseline_mentions']})")
    else:
        lines.append("\nTrending: no significant changes detected")

    # Stats
    m = graph.get("meta", {})
    lines.append(f"\nGraph: {m.get('total_nodes', 0)} nodes, "
                 f"{m.get('total_edges', 0)} edges")
    lines.append("=" * 50)

    return "\n".join(lines)


# ─────────────────────────────────────────────────────────────────────────────
#  CLI
# ─────────────────────────────────────────────────────────────────────────────

def _print_table(rows: List[dict], columns: List[str], widths: List[int]):
    fmt = "  ".join(f"{{:<{w}}}" for w in widths)
    print(fmt.format(*columns))
    print("  ".join("-" * w for w in widths))
    for row in rows:
        print(fmt.format(*[str(row.get(c, ""))[:w] for c, w in zip(columns, widths)]))


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Harvey Brain Knowledge Graph")
    parser.add_argument("--build", action="store_true", help="Build/rebuild graph from Brain")
    parser.add_argument("--god-nodes", action="store_true", help="Show most-referenced entities")
    parser.add_argument("--communities", action="store_true", help="Show knowledge communities")
    parser.add_argument("--neighbors", metavar="ENTITY", help="Show neighbors of entity")
    parser.add_argument("--path", nargs=2, metavar=("FROM", "TO"), help="Find path between entities")
    parser.add_argument("--stats", action="store_true", help="Graph summary stats")
    parser.add_argument("--context", metavar="QUERY", help="Graph context for a query")
    parser.add_argument("--top", type=int, default=15, help="Number of results (default: 15)")
    args = parser.parse_args()

    graph = get_or_build_graph(force=args.build)

    if args.build or args.stats:
        m = graph["meta"]
        print(f"\n{'='*50}")
        print(f"  Harvey Brain Knowledge Graph")
        print(f"{'='*50}")
        print(f"  Nodes:    {m['total_nodes']:>6}")
        print(f"  Edges:    {m['total_edges']:>6}")
        print(f"  Pages:    {m['pages']:>6}")
        print(f"  Journals: {m['journals']:>6}")
        print(f"  Saved:    {GRAPH_PATH}")
        print(f"{'='*50}\n")

    elif args.god_nodes:
        nodes = god_nodes(graph, top_n=args.top)
        print(f"\nTop {args.top} God Nodes (most referenced entities)\n")
        _print_table(nodes, ["name", "mentions", "type"], [40, 10, 12])

    elif args.communities:
        comms = communities(graph)
        print(f"\nKnowledge Communities\n")
        for c in comms:
            print(f"[{c['label']}] — {c['size']} nodes, cohesion={c['cohesion_score']}")
            print(f"  Members: {', '.join(c['members'][:8])}")
            print()

    elif args.neighbors:
        nb = neighbors(graph, args.neighbors)
        print(f"\nNeighbors of '{args.neighbors}'\n")
        print("Outgoing (links to):")
        for e in nb["outgoing"][:args.top]:
            print(f"  → {e['entity']} (weight={e['weight']})")
        print("\nIncoming (referenced by):")
        for e in nb["incoming"][:args.top]:
            print(f"  ← {e['entity']} (weight={e['weight']})")

    elif args.path:
        src, tgt = args.path
        path = find_path(graph, src, tgt)
        if path:
            print(f"\nPath: {' → '.join(path)}")
        else:
            print(f"\nNo path found between '{src}' and '{tgt}' within 4 hops")

    elif args.context:
        ctx = graph_context_for_query(graph, args.context)
        print(ctx or "No matching entities found")

    else:
        parser.print_help()
