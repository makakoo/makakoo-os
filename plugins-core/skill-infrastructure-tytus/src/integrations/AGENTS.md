## Tytus Private AI Pod

Private, encrypted OpenAI-compatible AI gateway with 383+ models.

### Setup
```bash
eval $(tytus env --export)     # Load TYTUS_AI_GATEWAY, TYTUS_API_KEY
export OPENAI_API_KEY=$TYTUS_API_KEY
export OPENAI_BASE_URL=${TYTUS_AI_GATEWAY}/v1
```

### Prerequisites
- Logged in: `tytus login` (one-time)
- Tunnel running: `sudo tytus connect` (in separate terminal)

### Commands
```bash
tytus status --json            # Pod and plan info
tytus env --json               # Connection details
tytus env --export             # Shell-sourceable exports
sudo tytus connect             # Allocate pod + tunnel (blocks until Ctrl+C)
tytus revoke <pod_id>          # Free pod units
tytus infect [dir]             # Drop integration files for all AI CLIs
tytus mcp                      # Print MCP server config
```

### API (OpenAI-compatible)
- Gateway: `$TYTUS_AI_GATEWAY/v1`
- Auth: `Bearer $TYTUS_API_KEY`
- Models: 383+ (run models endpoint to list)

### MCP Server
Configure `tytus-mcp` for native tool access:
- `tytus_status`, `tytus_env`, `tytus_models`, `tytus_chat`, `tytus_revoke`, `tytus_setup_guide`
