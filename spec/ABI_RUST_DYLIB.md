# ABI: Rust Dylib Plugin — v0.1 (draft)

**Status:** v0.1 DRAFT — 2026-04-21
**Kind:** `rust-dylib`
**Owner:** Makakoo kernel, `makakoo-core/src/abi/rust_dylib.rs` (NOT YET IMPLEMENTED)
**Promotes to v1.0:** after v0.3 lands a reference implementation

This spec exists so v0.3+ can build native Rust plugins against a stable
contract without renegotiating the ABI. **No kernel code ships in v0.2 that
loads rust-dylib plugins** — Phase A.5 of the v0.2 "Harden & Connect" sprint
is spec-only.

---

## 0. Why dylibs?

Every current plugin kind (`skill`, `agent`, `sancho-task`, `library`,
`mascot`) runs out-of-process — subprocess for Python entrypoints, separate
service for long-lived agents. That's the right default for three reasons:
language interop, crash isolation, and `cargo install` churn.

But some extension points are hot enough that spawning a subprocess every
call destroys the latency budget. Ideas the rust-dylib ABI will eventually
support:

- Custom MCP tool handlers that fire >10 Hz (e.g. a real-time
  telemetry collector).
- Capability-level middleware (e.g. a pre-flight "confirm destructive
  command" gate signed by a remote key).
- Brain-layer indexers that must share the kernel's SQLite connection
  pool without IPC copies.
- Router LLM callbacks running inline with request dispatch.

The common thread: sub-ms call paths where the fork cost of a subprocess
would dominate.

## 1. Who should NOT use this

If any of these apply, ship a normal subprocess plugin instead:

- Your plugin is written in a language other than Rust.
- Your plugin shells out to CLIs, reads files, or calls LLMs as its main
  job (subprocess latency is already dominated by those calls).
- Your plugin is a one-shot tool (no steady call rate).
- Your plugin needs to survive a kernel crash — dylibs die with the
  process.
- Your plugin wants to call `cargo` / `rustc` at runtime — you can't.

Dylibs buy latency. Everything else costs you process isolation and
upgrade convenience.

## 2. Manifest

```toml
[plugin]
name = "plugin-rust-example"
version = "0.1.0"
kind = "rust-dylib"
language = "rust"
summary = "In-process Rust plugin skeleton"

[source]
path = "plugins-core/plugin-rust-example"

[abi]
# Required. Exactly one rust-dylib version the plugin compiles against.
# The kernel refuses to load anything outside its supported range.
rust-dylib = "^1.0"

# Required for every extension point the dylib implements. See §4.
# The kernel also checks the dylib exports match these advertisements
# at load time.
rust-dylib-capabilities = ["mcp-tool"]

[build]
# Required. How the kernel (re)builds the dylib. The kernel invokes this
# command from `$MAKAKOO_HOME/plugins/<plugin-name>/` and expects the
# output at the path in `[source.rust-dylib]`.
unix = "cargo build --release --manifest-path Cargo.toml"
windows = "cargo build --release --manifest-path Cargo.toml"

[source.rust-dylib]
# Relative to plugins/<plugin-name>/. The artifact the kernel dlopens.
# macOS: .dylib; Linux: .so; Windows: .dll. The kernel auto-picks the
# right extension based on host OS.
library = "target/release/libplugin_rust_example"

[capabilities]
grants = ["brain/read"]
```

## 3. Crate setup

The plugin's Cargo.toml must:

```toml
[package]
name = "plugin-rust-example"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
makakoo-plugin-sdk = "1.0"
```

`makakoo-plugin-sdk` (new crate, shipped alongside `makakoo-core`) re-exports
the `#[plugin_entry]` proc-macro + the ABI types the dylib sees. No other
`makakoo-*` crate is directly linkable by plugins — `makakoo-core` is
kernel-private.

## 4. Entry point

Every dylib exports exactly one symbol:

```rust
use makakoo_plugin_sdk::{plugin_entry, PluginEntry, ExtensionPoint};

#[plugin_entry]
pub fn register(entry: &mut PluginEntry) {
    entry
        .name("plugin-rust-example")
        .version(env!("CARGO_PKG_VERSION"))
        .on_mcp_tool("example_tool", example_tool);
}

async fn example_tool(ctx: ExtensionPoint, params: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::json!({ "echoed": params }))
}
```

At load time the kernel:

1. `dlopen`s the library path from `[source.rust-dylib].library`.
2. Looks up the symbol `_makakoo_plugin_entry_v1` that `#[plugin_entry]`
   generates.
3. Calls it with a zero-initialized `PluginEntry`.
4. Asserts the builder state matches `[abi]
   rust-dylib-capabilities = [...]` from the manifest — a plugin that
   advertises `mcp-tool` but registers no MCP tools is a hard error.
5. Stores the populated `PluginEntry` for dispatch.

## 5. Extension points (v0.1 scope)

Only `mcp-tool` is in v0.1. Everything else is reserved.

| Manifest name | Trait                    | Dispatch site                      |
|---------------|--------------------------|------------------------------------|
| `mcp-tool`    | `AsyncMcpTool`           | `makakoo-mcp::handlers`             |
| `sancho-task` | reserved (v0.2 deferred) | —                                  |
| `capability-gate` | reserved (v0.3+)     | —                                  |
| `telemetry-sink`  | reserved (v0.3+)     | —                                  |

Adding a new extension point is a breaking change that bumps the ABI
MAJOR.

## 6. Capability hooks

Dylibs live inside the kernel process — they inherit all the kernel's
privileges. The only gate keeping them in line is the manifest's
`[capabilities].grants` list. The kernel enforces grants at **every**
`ExtensionPoint` API call that crosses an audit boundary:

- `ExtensionPoint::brain_read(query)` — requires `brain/read`.
- `ExtensionPoint::brain_write_journal(entry)` — requires `brain/write`.
- `ExtensionPoint::outbound_draft(channel, …)` — requires `outbound/<channel>/draft`.
- `ExtensionPoint::llm_chat(model, messages)` — requires `llm/chat`.

Calls without the right grant return `CapabilityError::Denied`. There is
**no escape hatch**. A dylib that wants to read files directly must
request `fs/read:<path>` just like any other plugin.

## 7. ABI stability

| Field            | Guarantee                                                  |
|------------------|------------------------------------------------------------|
| `PluginEntry`    | repr(C), fixed layout, fields are append-only.             |
| `ExtensionPoint` | Trait object, vtable ABI preserved across MINOR.           |
| `AsyncMcpTool`   | Same trait object guarantees.                              |
| rust-dylib MAJOR | Breaking. Kernel refuses to load plugins built against a different MAJOR. |
| rust-dylib MINOR | Additive. Older plugin binaries keep working.              |
| rust-dylib PATCH | Bug fix only. Binary-compatible.                           |

## 8. Loading / unloading

- The kernel loads every `kind = "rust-dylib"` plugin at startup after
  the plugin registry finishes (`PluginRegistry::load_default`).
- Unloading is **not supported** in v0.1 — dylibs live for the kernel's
  lifetime. `plugin uninstall` removes the manifest but the dylib stays
  memory-resident until the kernel restarts. This mirrors how tracing
  subscribers / native extensions behave in other languages.
- `plugin reload` schedules a kernel restart. Live-patching dylibs is
  out of scope for v0.1.

## 9. Compatibility table

| Host Rust MSRV | `makakoo-plugin-sdk` MSRV | Compatible plugin Rust | Notes                                   |
|----------------|---------------------------|-------------------------|------------------------------------------|
| 1.80           | 1.80                      | 1.80+                   | Every plugin must match host `edition`.  |
| 1.81           | 1.80                      | 1.80 / 1.81             |                                          |

If the plugin SDK ever adopts `edition = "2024"`, that's a MAJOR bump.

## 10. Open questions (resolve before v0.3)

1. **Symbol versioning.** Should `_makakoo_plugin_entry_v1` encode the
   entire rust-dylib version in the symbol name, or only MAJOR? (Current
   proposal: MAJOR only.)
2. **Crash isolation.** A dylib panic aborts the kernel today. Worth
   catching via `catch_unwind` for MCP-tool handlers specifically?
   (Proposed yes; panics in other extension points stay fatal.)
3. **Build reproducibility.** Should the kernel verify the built
   artifact's hash against `plugins.lock`? (Proposed yes, via the
   existing PluginsLock format with a `rust_dylib_sha256` field.)
4. **Cross-compile.** Do we ship prebuilt dylibs in the Homebrew/winget
   artifacts or make everyone build from source? (Proposed: source-only
   in v0.3; prebuilts in v0.4 once signing infra covers native libs.)

## 11. Review

Route this document through `lope negotiate` before promoting to v1.0.
Required reviewer agreement: ≥ 2 PASS from the ensemble
`{claude, gemini, opencode}`. Explicit concerns to surface:

- Does `#[plugin_entry]` collide with popular Rust ecosystem crates?
- Is `cdylib` the right crate-type (vs. `dylib`)? cdylib is C-ABI clean;
  dylib allows Rust-internal types to cross.
- Should the SDK pin `tokio = "1.x"` or leave it abstract behind a trait?
- Should `makakoo-plugin-sdk` re-export `serde_json::Value` or have its
  own `PluginValue` wrapper?

## 12. Non-goals (v0.1)

- Hot reload of dylibs without kernel restart.
- Isolated address spaces (use WASM plugins for that; see ABI_WASM.md in
  a future sprint).
- Sandboxing beyond the capability layer.
- Generic / type-parameterized extension points. Trait objects only.
- Language interop within the dylib: a rust-dylib plugin that internally
  calls into cpp via FFI is fine, but the plugin's contract with the
  kernel stays pure Rust.
