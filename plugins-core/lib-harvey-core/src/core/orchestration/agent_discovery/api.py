"""
Agent Discovery HTTP API Server
"""

import json
import os
from datetime import datetime, timezone, timedelta
from http.server import HTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse, parse_qs
import threading
import logging

from .store import RegistryStore
from .models import AgentRecord

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger("agent_discovery.api")

DEFAULT_PORT = 18081


class DiscoveryHandler(BaseHTTPRequestHandler):
    store: RegistryStore = None

    def _send_json(self, status: int, data: dict) -> None:
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(data).encode())

    def _read_json(self) -> dict:
        length = int(self.headers.get("Content-Length", 0))
        if length == 0:
            return {}
        return json.loads(self.rfile.read(length))

    def do_GET(self) -> None:
        parsed = urlparse(self.path)
        path = parsed.path
        query = parse_qs(parsed.query)

        if path == "/agents":
            cap = query.get("capability", [None])[0]
            agents = self.store.list(capability=cap)
            self._send_json(200, {
                "agents": [a.to_dict() for a in agents],
                "count": len(agents),
            })

        elif path.startswith("/agents/") and len(path.split("/")) == 3:
            agent_id = path.split("/")[-1]
            agent = self.store.get(agent_id)
            if agent:
                self._send_json(200, agent.to_dict())
            else:
                self._send_json(404, {"error": "Agent not found"})

        elif path == "/agents/health":
            stats = self.store.health_stats()
            self._send_json(200, stats)

        elif path == "/health":
            self._send_json(200, {"status": "ok", "service": "agent-discovery"})

        else:
            self._send_json(404, {"error": "Not found"})

    def do_POST(self) -> None:
        parsed = urlparse(self.path)
        path = parsed.path

        if path == "/agents/register":
            body = self._read_json()
            try:
                record = AgentRecord(
                    agent_id=body["agent_id"],
                    name=body["name"],
                    capabilities=body.get("capabilities", []),
                    skills=body.get("skills", []),
                    endpoint=body.get("endpoint", ""),
                    metadata=body.get("metadata", {}),
                )
                ttl = body.get("ttl_seconds", 300)
                self.store.register(record, ttl_seconds=ttl)
                self._send_json(200, {
                    "status": "registered",
                    "lease_expires_at": record.lease_expires_at,
                })
            except KeyError as e:
                self._send_json(400, {"error": f"Missing required field: {e}"})

        elif path.startswith("/agents/heartbeat/"):
            agent_id = path.split("/")[-1]
            body = self._read_json()
            ttl = body.get("ttl_seconds", 300)
            success = self.store.refresh_lease(agent_id, ttl)
            if success:
                self._send_json(200, {"status": "ok"})
            else:
                self._send_json(404, {"error": "Agent not found"})

        else:
            self._send_json(404, {"error": "Not found"})

    def do_DELETE(self) -> None:
        parsed = urlparse(self.path)
        path = parsed.path

        if path.startswith("/agents/") and len(path.split("/")) == 3:
            agent_id = path.split("/")[-1]
            if self.store.deregister(agent_id):
                self._send_json(200, {"status": "deregistered"})
            else:
                self._send_json(404, {"error": "Agent not found"})
        else:
            self._send_json(404, {"error": "Not found"})

    def log_message(self, format, *args) -> None:
        logger.info(f"{self.address_string()} {format % args}")


def run_server(port: int = DEFAULT_PORT, db_path: Optional[str] = None) -> None:
    """Run the discovery API server.""", List

    data_dir = os.environ.get("HARVEY_DATA", os.path.join(os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO")), "data"))
    if db_path is None:
        db_path = f"{data_dir}/agent_discovery/registry.db"

    # Ensure directory exists
    os.makedirs(os.path.dirname(db_path), exist_ok=True)

    store = RegistryStore(db_path)
    DiscoveryHandler.store = store

    server = HTTPServer(("0.0.0.0", port), DiscoveryHandler)
    logger.info(f"Agent Discovery API running on port {port}")
    server.serve_forever()


if __name__ == "__main__":
    run_server()
