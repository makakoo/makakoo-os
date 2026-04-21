# Quickstart Guide

Get productive with Makakoo OS in 15 minutes.

## Your First Day

### 1. Ask Your Brain

Open Claude Code (or any infected CLI):

```
> What did I work on yesterday?
```

Makakoo searches your journals and provides an answer.

### 2. Write to Your Brain

```
> Remember I decided to use PostgreSQL for the main database
```

This gets written to today's journal automatically.

### 3. Create a Project Page

```bash
# Create a project page
cat > ~/MAKAKOO/Brain/pages/projects/my-app.md << 'EOF'
# My App

## Status
Planning

## Tech Stack
- Backend: PostgreSQL
- Frontend: React
- AI: Claude Code

## Decisions
- [x] Chose PostgreSQL (2026-04-20)
EOF
```

### 4. Search Everything

```bash
# Find something you mentioned
makakoo search "polymarket"

# Ask a complex question
makakoo query "what are my active trading strategies?"
```

---

## Common Workflows

### Morning: Check Your Brain

```bash
# Get daily briefing
makakoo query "what should I focus on today?"
```

### During Work: Remember Decisions

```
> Remember I'm using the new API endpoint
> Store that the retry limit is 3 attempts
> Add to the architecture doc that we chose microservices
```

### Evening: Review Progress

```bash
# Search what you worked on
makakoo search "today"

# Ask for summary
makakoo query "summarize my work this week"
```

---

## Key Commands

### Essential

```bash
# Ask anything
makakoo query "your question"

# Search
makakoo search "keywords"

# Write to journal (automatic when AI mentions)
```

### Daily Maintenance

```bash
# Check system health
makakoo health

# View running tasks
makakoo sancho status

# Check plugins
makakoo plugin list
```

### Exploration

```bash
# See your journals
ls ~/MAKAKOO/Brain/journals/

# Browse pages
ls ~/MAKAKOO/Brain/pages/

# Read a journal
cat ~/MAKAKOO/Brain/journals/$(date +%Y_%m_%d).md
```

---

## Tips for Effective Use

### 1. Journal Frequently

The more you write to journals, the smarter Makakoo becomes:

```
> Remember that X is true
> I decided Y
> Note that Z needs follow-up
```

### 2. Create Pages for Projects

Pages are structured and searchable:

```
~/MAKAKOO/Brain/pages/
├── projects/
│   ├── my-project.md
│   └── other-project.md
├── decisions/
│   └── 2026-04-architecture.md
└── notes/
    └── meetings.md
```

### 3. Use Tags

Makakoo understands tags:

```markdown
# My Project

## Status
#status/in-progress

## Tags
#project/my-app #team/backend
```

### 4. Ask Follow-up Questions

Makakoo remembers context:

```
> What did I decide about the database?
> Why did I make that choice?
> What are the alternatives I considered?
```

---

## Example Session

```bash
$ claude

> What projects am I working on?
Makakoo searches your brain...

> Remember I'm starting a new project called "Atlas"
Makakoo creates a journal entry...

> Create a page at ~/MAKAKOO/Brain/pages/projects/atlas.md
Makakoo creates the file...

> What do I know about AI agents?
Makakoo synthesizes from all your notes...
```

---

## Next Steps

- [Installation Guide](getting-started.md) — If you haven't installed yet
- [Brain Guide](brain/index.md) — Deep dive into memory
- [Plugin Guide](plugins/index.md) — Add new capabilities
- [Troubleshooting](troubleshooting/index.md) — Common issues
