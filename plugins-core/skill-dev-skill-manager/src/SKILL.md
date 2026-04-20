---
name: skill-manager
description: Create, patch, edit, delete, and manage Harvey OS skills. Use when building new skills from successful approaches, fixing skill issues during use, or maintaining the skill library.
version: 1.0.0
author: Harvey Agent
platforms: [macos, linux]
---

# Skill Manager

Autonomous skill creation and management tool for Harvey OS.

## When to Use

**Create a skill when:**
- A complex task succeeded after 5+ tool calls
- Errors were overcome through trial and error
- A non-trivial workflow was discovered
- The user asks you to remember a procedure
- A pattern emerges that could be reused

**Update (patch) a skill when:**
- Instructions are stale or wrong
- OS-specific failures are discovered
- Missing steps or pitfalls found during use
- The skill should be patched immediately when issues are found

**Do NOT create skills for:**
- Simple one-off tasks
- Trivial queries

## Actions

### create

Create a new skill with full SKILL.md content including YAML frontmatter.

```json
{
  "action": "create",
  "name": "my-new-skill",
  "category": "dev",
  "content": "---\nname: my-new-skill\ndescription: Brief description\n---\n\n# My New Skill\n\nContent..."
}
```

### patch

Targeted find-and-replace within SKILL.md or a supporting file. Preferred for fixes — minimal and precise.

```json
{
  "action": "patch",
  "name": "existing-skill",
  "old_string": "old text to find",
  "new_string": "replacement text",
  "replace_all": false
}
```

### edit

Full rewrite of SKILL.md. Use for major overhauls only.

```json
{
  "action": "edit",
  "name": "existing-skill",
  "content": "---\nname: existing-skill\n...\n"
}
```

### delete

Remove a skill entirely.

```json
{
  "action": "delete",
  "name": "skill-to-delete"
}
```

### write_file

Add or overwrite a supporting file (references/, templates/, scripts/, assets/).

```json
{
  "action": "write_file",
  "name": "my-skill",
  "file_path": "references/api-guide.md",
  "file_content": "# API Guide\n\nContent..."
}
```

### remove_file

Remove a supporting file from a skill.

```json
{
  "action": "remove_file",
  "name": "my-skill",
  "file_path": "references/old-guide.md"
}
```

## SKILL.md Format

All skills must have YAML frontmatter:

```yaml
---
name: skill-name
description: Brief description (max 1024 chars)
version: 1.0.0
author: Harvey Agent
platforms: [macos, linux]  # Optional
---

# Skill Title

Content with trigger conditions, numbered steps, pitfalls, and verification steps.
```

## Safety

- YAML frontmatter validated on every write (name + description required)
- Atomic writes with rollback on failure
- Security scan on every write (prompt injection patterns)
- Path traversal protection
- Name collision detection
- Max name: 64 chars, lowercase, hyphens/dots/underscores only

## Directory Structure

```
harvey-os/skills/<category>/
└── <skill-name>/
    ├── SKILL.md
    ├── references/
    ├── templates/
    ├── scripts/
    └── assets/
```
