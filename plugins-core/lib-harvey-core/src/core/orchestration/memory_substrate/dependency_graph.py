"""
Dependency Graph — Artifact dependency DAG with cycle detection.
"""


class DependencyGraph:
    """Track artifact dependency DAG. Detect cycles."""

    def __init__(self):
        self.graph: dict[str, list[str]] = {}  # artifact_id -> list of depends_on

    def add(self, artifact_id: str, depends_on: list[str]):
        """Add an artifact with its dependencies. Raises if cycle detected."""
        self.graph[artifact_id] = list(depends_on)
        if self.has_cycle():
            # Rollback
            del self.graph[artifact_id]
            raise ValueError(f"Cycle detected when adding {artifact_id}")

    def remove(self, artifact_id: str):
        """Remove an artifact from the graph."""
        if artifact_id not in self.graph:
            return
        del self.graph[artifact_id]
        # Remove from all dependency lists
        for deps in self.graph.values():
            if artifact_id in deps:
                deps.remove(artifact_id)

    def has_cycle(self) -> bool:
        """Detect if the graph has a cycle using DFS."""
        visited: set[str] = set()
        path: set[str] = set()

        def dfs(node: str) -> bool:
            if node in path:
                return True  # cycle found
            if node in visited:
                return False
            visited.add(node)
            path.add(node)
            for dep in self.graph.get(node, []):
                if dfs(dep):
                    return True
            path.remove(node)
            return False

        return any(dfs(n) for n in self.graph)

    def get_dependents(self, artifact_id: str) -> list[str]:
        """Get all artifacts that depend on this artifact."""
        return [
            art_id for art_id, deps in self.graph.items() if artifact_id in deps
        ]

    def topological_sort(self) -> list[str]:
        """Return artifacts in topological order (dependencies first)."""
        in_degree = {art_id: 0 for art_id in self.graph}
        for deps in self.graph.values():
            for dep in deps:
                in_degree[dep] = in_degree.get(dep, 0)

        queue = [art_id for art_id, deg in in_degree.items() if deg == 0]
        result = []
        while queue:
            node = queue.pop(0)
            result.append(node)
            for dep in self.graph.get(node, []):
                in_degree[dep] -= 1
                if in_degree[dep] == 0:
                    queue.append(dep)
        return result
