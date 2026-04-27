# `makakoo adapter`

Manage LLM gateway adapters — the plugins that route Makakoo's internal calls to an OpenAI-compatible backend (switchAILocal, Anthropic direct, OpenRouter, pi's tytus pod, etc.). Paired with the `model-provider` section of `makakoo setup` for picking a primary.

## Example

```sh
makakoo adapter list
makakoo adapter install <source>
makakoo adapter enable <name>
makakoo adapter doctor <name>
```

## Full command surface

Run with `--help` for every flag, subcommand, and exit code:

```sh
makakoo adapter --help
```

See also: [User manual index](index.md), [Use cases](../use-cases.md), [Troubleshooting](../troubleshooting/index.md).
