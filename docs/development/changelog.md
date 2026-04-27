# Changelog

All notable changes to Makakoo OS.

For the complete changelog, see [CHANGELOG.md](../../CHANGELOG.md) in the project root.

## Quick Links

- [v0.1.0 Release Notes](../RELEASE_NOTES_0.1.0.md) — What's new in v0.1.0
- [Migration Status](../MIGRATION-STATUS.md) — Upgrading from older versions

## Version Overview

| Version | Date | Status |
|---------|------|--------|
| v0.1.0 | YYYY-MM-DD | Latest |
| v0.0.x | Earlier | Deprecated |

## How We Track Changes

We follow [Semantic Versioning](https://semver.org/):

- **Major** (v1.0.0) — Breaking changes
- **Minor** (v0.2.0) — New features, backward compatible
- **Patch** (v0.1.1) — Bug fixes, backward compatible

## Change Categories

| Prefix | Meaning |
|--------|---------|
| `feat` | New feature |
| `fix` | Bug fix |
| `docs` | Documentation |
| `refactor` | Code refactor |
| `test` | Tests |
| `chore` | Maintenance |

## Contributing to Changelog

When adding changes:

```bash
# After your fix/feature
git commit -m "feat(brain): add wikilink support"
```

The changelog is auto-generated from commit messages on release.

## Release Notes by Version

### v0.1.0 (Latest)

[View full release notes](RELEASE_NOTES_0.1.0.md)

**Highlights:**
- Initial release
- Infection for 7+ AI CLIs
- Persistent Brain with journals and pages
- Superbrain search (FTS5 + vectors + LLM)
- SANCHO proactive task engine
- 38 built-in plugins
- Capability sandboxing
- macOS, Linux, Windows support

---

### Pre-release (Unreleased)

See [CHANGELOG.md](../../CHANGELOG.md) for all pre-release changes.

## Deprecation Notices

### Plugins

When a plugin is deprecated:

1. Warning added to `plugin info`
2. Listed in deprecation notice
3. Removed after 2 minor versions

### Capabilities

When a capability verb changes:

1. Old verb deprecated with warning
2. Migration guide provided
3. Old verb removed after 1 major version

## Breaking Changes

### v0.1.0 → v0.2.0 (Planned)

TBD - no breaking changes currently planned.

## See Also

- [Contributing Guide](./contributing.md) — How to contribute
- [Architecture](../concepts/architecture.md) — System design
- [Spec Documents](../../spec/) — ABI contracts
