# Security Assessment — "Is this safe?"

## Overview

Interactive security wizard that classifies command risk and provides plain-english explanations of security implications.

## Features

- **Natural language risk classification**: Ask "what does this do?" and get a risk level
- **Risk levels**: LOW | MEDIUM | HIGH | FORBIDDEN
- **Detailed explanations**: Understand why something is risky
- **Dangerous command detection**: Catches fork bombs, rm -rf, SQL injection patterns, etc.
- **Alternative suggestions**: Offers safer ways to accomplish the goal

## Requirements

- Python 3.8+
- Wraps existing: `core.security.risk_classifier`, `core.security.dangerous_command`
- No external dependencies

## Data Volume

Results are logged to: `$HARVEY_HOME/data/security-audits/`
- Assessment logs: `assessments_YYYY_MM_DD.json`
- Dangerous command history: `dangerous_commands.log`

## Execution

```bash
# Run wizard
python3 -m skills.system.security.security_wizard

# Or direct invocation
cd $HARVEY_HOME/harvey-os
python3 -c "from skills.system.security.security_wizard import security_wizard; security_wizard()"
```

## Integration

Can be called programmatically:

```python
from skills.system.security.security_wizard import check_command_safety

is_safe = check_command_safety('rm -rf /')  # Returns True if safe, False if user declines
```

## Example Output

```
╔════════════════════════════════════════╗
║  Security Assessment                   ║
║  Is this safe?                          ║
╚════════════════════════════════════════╝

Step 1/1

? Describe the action in plain English
> delete all files in /var

[high] This action has significant risks:
  • Could affect multiple systems
  • Requires careful consideration
  • Should be done with caution

Proceed with caution? [y/N]: n
Action cancelled.
```

## Risk Classifications

### LOW
✅ Safe for most users without additional review.

Examples:
- View files (cat, grep, find)
- Check system info (ps, top, uname)

### MEDIUM
⚠️ Should be reviewed before execution. May have unintended side effects.

Examples:
- Create/modify files (touch, echo, sed)
- Install packages (pip, npm, brew)
- Network operations (curl, wget)

### HIGH
🚫 Significant risk. Requires careful consideration and user approval.

Examples:
- Delete files (rm, rmdir)
- Modify permissions (chmod, chown)
- Stop services (kill, systemctl stop)

### FORBIDDEN
❌ Blocked for safety. Contact administrator if necessary.

Examples:
- Fork bombs (:(){ :|:& };:)
- Destructive patterns (dd if=/dev/zero of=/dev/sda)
- Privilege escalation attempts

## Logging

All security assessments are logged to Brain:
```markdown
- [[Security]] assessed command: "delete all files in /var"
  - Risk level: HIGH
  - Timestamp: 2026-04-10T12:34:56Z
  - User decision: REJECTED
```
