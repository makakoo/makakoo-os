# Contributing to Makakoo OS

We welcome contributions of all kinds.

## Ways to Contribute

- **Bug Reports** — Find something broken? Open an issue.
- **Feature Requests** — Have an idea? Share it.
- **Documentation** — Improve docs for everyone.
- **Plugins** — Share your plugins with the community.
- **Code** — Fix bugs, add features.

## Getting Started

### 1. Fork and Clone

```bash
# Fork on GitHub, then clone your fork
git clone https://github.com/YOUR_USERNAME/makakoo-os
cd makakoo-os
```

### 2. Install Dependencies

```bash
# Install Rust (if needed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Build
cargo build --release

# Run tests
cargo test --workspace
```

### 3. Create a Branch

```bash
git checkout -b feature/my-feature
# or
git checkout -b fix/my-bug
```

## Development Workflow

### Code Style

We use `cargo fmt` and `cargo clippy`:

```bash
# Format code
cargo fmt

# Lint
cargo clippy -- -D warnings

# Both (before every commit)
cargo fmt && cargo clippy -- -D warnings
```

### Commit Messages

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add new plugin type
fix: correct capability check
docs: update installation guide
test: add tests for brain module
refactor: simplify SANCHO scheduler
```

### Pull Request Process

1. **Fork** the repository
2. **Create** a feature branch
3. **Make** your changes
4. **Test** thoroughly
5. **Submit** PR with description
6. **Address** review feedback

## Project Structure

```
makakoo-os/
├── makakoo/              # CLI binary
├── makakoo-core/         # Core library
├── makakoo-mcp/          # MCP server
├── makakoo-platform/     # Platform integrations
├── makakoo-client/       # Rust client
├── makakoo-client-py/    # Python client
├── plugins-core/         # Built-in plugins
├── distros/              # Distro bundles
├── spec/                 # ABI contracts
└── docs/                # Documentation
```

## Testing

### Run All Tests

```bash
cargo test --workspace
```

### Run Specific Tests

```bash
cargo test -p makakoo-core
cargo test -p makakoo
```

### Integration Tests

```bash
# Build
cargo build --release

# Test installation
./target/release/makakoo install --dry-run
```

## Documentation

### Building Docs

Docs are Markdown files in `docs/`:

```
docs/
├── index.md
├── getting-started.md
├── user-manual/
├── plugins/
└── troubleshooting/
```

### Doc Guidelines

- Use clear headings
- Include code examples
- Add screenshots for UI parts
- Link related docs

## Issue Guidelines

### Bug Reports

```markdown
**Description**
Clear description of the bug.

**Steps to Reproduce**
1. Go to '...'
2. Click on '...'
3. See error

**Expected Behavior**
What should happen.

**Environment**
- OS: macOS/Linux/Windows
- Version: v0.1.0
```

### Feature Requests

```markdown
**Problem**
What problem does this solve?

**Proposed Solution**
How should it work?

**Alternatives Considered**
What else did you consider?
```

## Plugin Development

See [Writing Plugins](../plugins/writing.md) for full guide.

### Quick Start

```bash
# Create plugin structure
mkdir my-plugin
cd my-plugin
cat > plugin.toml << 'EOF'
[plugin]
name = "my-plugin"
version = "0.1.0"
kind = "skill"
summary = "My plugin"

[source]
path = "."

[abi]
skill = "^0.1"
EOF
```

## Community

### Discord

Join our [Discord server](https://discord.gg/makakoo) for:
- Questions
- Ideas
- Show & tell
- Help

### GitHub Discussions

Use [Discussions](https://github.com/makakoo/makakoo-os/discussions) for:
- RFC proposals
- Q&A
- Announcements

## License

By contributing, you agree that your contributions will be licensed under the MIT License.

## Code of Conduct

- Be respectful
- Be inclusive
- Be helpful
- Focus on what's best for the community

## Recognition

All contributors are recognized in:
- CHANGELOG.md
- CONTRIBUTORS.md
- Release notes

## Questions?

- Open an [Issue](https://github.com/makakoo/makakoo-os/issues)
- Ask in [Discord](https://discord.gg/makakoo)
- Check [Discussions](https://github.com/makakoo/makakoo-os/discussions)
