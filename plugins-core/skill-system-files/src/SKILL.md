# File Manager — "Show me my files"

## Overview

Interactive file browser with arrow-key navigation for browsing, previewing, copying, and safely deleting files.

## Features

- **Arrow-key navigation**: Browse directories intuitively
- **File preview**: Peek inside text files (JSON, YAML, Markdown, etc.)
- **Safe delete**: Confirmation and size preview before deletion
- **Copy files**: Copy files and folders to new locations
- **File size display**: Human-readable sizes (B, KB, MB, GB)
- **Permission handling**: Graceful errors for inaccessible directories

## Requirements

- Python 3.8+
- Standard library only (os, shutil, pathlib)
- No external dependencies

## Data Volume

File manager logs are written to: `$HARVEY_HOME/data/file-manager/`
- Recent locations: `recent_paths.json`
- Deleted files log: `deleted_YYYY_MM_DD.log`

## Execution

```bash
# Run file manager
python3 -m skills.system.files.file_manager

# Or direct invocation
cd $HARVEY_HOME/harvey-os
python3 -c "from skills.system.files.file_manager import file_manager_main; file_manager_main()"
```

## Integration

Can be called programmatically:

```python
from skills.system.files.file_manager import FileManager

manager = FileManager(start_path='~')
manager.browse()
```

## Keyboard Controls

| Key | Action |
|---|---|
| ↑↓ | Navigate up/down |
| Enter | Open directory / Perform action on file |
| ..  | Go to parent directory |
| v | View/preview file contents |
| d | Delete file (with confirmation) |
| c | Copy file to new location |
| ? | Show help |
| q/Esc | Close and go back |

## Example Output

```
╔════════════════════════════════════════╗
║  File Manager                           ║
║  /Users/sebastian/MAKAKOO               ║
╚════════════════════════════════════════╝

Navigate /Users/sebastian/MAKAKOO (use arrow keys, Enter to select):

  ► .. (Go to parent directory)
    📁 agents
    📁 data
    📁 harvey-os
    📄 README.md (2.5KB)
    📄 LICENSE (1.0KB)

(↑↓ navigate, Enter select, ? help)
```

## File Operations

### View File
Shows preview for text files (first 20 lines):
```
╔════════════════════════════════════════╗
║  config.json                            ║
╠════════════════════════════════════════╣
║ {                                       ║
║   "name": "Harvey OS",                  ║
║   "version": "1.0.0"                    ║
║ }                                       ║
╚════════════════════════════════════════╝
```

### Delete File
Confirmation with size:
```
⚠️  Delete config.json (2.5KB)? This cannot be undone.
[y/N]: y

[success] Deleted successfully
```

### Copy File
Relative path from current directory:
```
Copy to (relative to /Users/sebastian/MAKAKOO): backups/config.json.bak
[success] Copied successfully
```

## Safety Features

- **Confirmation required** for all destructive operations (delete)
- **Size preview** shown before deletion
- **Permission handling** — gracefully handles "access denied" errors
- **No force operations** — respects OS file protections
- **No hidden files by default** — shows everything, but user must navigate explicitly

## Logging

All file operations are logged:
```markdown
- [[Files]] deleted: /path/to/file.txt (2.5KB)
  - Timestamp: 2026-04-10T12:34:56Z
  - Reason: User request
  - Status: SUCCESS
```
