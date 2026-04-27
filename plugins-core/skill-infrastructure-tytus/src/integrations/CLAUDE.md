## Tytus Private AI Pod

This project has access to a Tytus private AI pod — an encrypted, OpenAI-compatible gateway with 383+ models.

### MCP Tools (if tytus MCP server configured)
- `tytus_status` — Check login state, plan, active pods
- `tytus_env` — Get connection URLs and API keys
- `tytus_models` — List available models on the pod
- `tytus_chat` — Send chat completions through private pod
- `tytus_setup_guide` — Step-by-step setup if not connected

### Manual usage
```bash
# Load connection vars
eval $(tytus env --export)

# List models
curl -s "$TYTUS_AI_GATEWAY/v1/models" -H "Authorization: Bearer $TYTUS_API_KEY" | jq '.data[].id'

# Chat
curl "$TYTUS_AI_GATEWAY/v1/chat/completions" \
  -H "Authorization: Bearer $TYTUS_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"qwen3-8b","messages":[{"role":"user","content":"hello"}]}'
```

### OpenAI-compatible env
```bash
export OPENAI_API_KEY=$TYTUS_API_KEY
export OPENAI_BASE_URL=${TYTUS_AI_GATEWAY}/v1
```

### Quick Reference
| What | Value |
|------|-------|
| Gateway | `$TYTUS_AI_GATEWAY/v1` |
| Auth | `Bearer $TYTUS_API_KEY` |
| Models | 383+ (run tytus_models or models endpoint) |
| Status | `tytus status` or `tytus_status` MCP tool |
| Connect | `sudo tytus connect` (separate terminal) |
| Infect project | `tytus infect` (drops all integration files) |
