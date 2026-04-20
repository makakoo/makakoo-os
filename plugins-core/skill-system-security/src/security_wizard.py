"""Security Wizard — "Is this command safe?" guided risk assessment."""

import sys
from pathlib import Path

# Add parent to path for HTE import
sys.path.insert(0, str(Path(__file__).parent.parent.parent.parent.parent))

from core.terminal import Wizard, WizardStep, Panel, StatusBadge, Header, Alert
from core.security.risk_classifier import classify_command_detailed, RiskLevel
from core.security.dangerous_command import detect_dangerous_command, prompt_dangerous_approval


def security_wizard():
    """Run interactive security assessment wizard."""
    print("\n" + Header("Security Assessment", "Is this safe?").render() + "\n")

    steps = [
        WizardStep(
            id="command",
            title="What are you trying to do?",
            prompt="Describe the action in plain English",
            help_text="For example: 'delete the file /tmp/test.txt' or 'install a Python package'",
            input_type="nl_prompt",
        ),
    ]

    wizard = Wizard("Security Assessment", steps)
    results = wizard.run()

    if not results:
        return

    command_desc = results.get("command", "")
    print("\n")

    # Classify risk
    try:
        risk_level, description = classify_command_detailed(command_desc)
    except Exception:
        # Fallback if classification fails
        risk_level = RiskLevel.MEDIUM
        description = "Unable to fully classify — treating as medium risk"

    # Show risk badge
    level_name = risk_level.name.lower()
    badge = StatusBadge(level_name).render()
    print(f"{badge} {description}\n")

    # Explain what this means
    if risk_level == RiskLevel.FORBIDDEN:
        print("This action is blocked for safety reasons:")
        print("  • May cause data loss")
        print("  • May compromise security")
        print("  • May affect system stability\n")
        print("Please contact your system administrator if you believe this is necessary.\n")

    elif risk_level == RiskLevel.HIGH:
        print("This action has significant risks:")
        print("  • Could affect multiple systems")
        print("  • Requires careful consideration")
        print("  • Should be done with caution\n")

        confirm = input(f"Proceed with caution? [y/N]: ").strip().lower()
        if confirm not in ("y", "yes"):
            print("Action cancelled.\n")
            return

    elif risk_level == RiskLevel.MEDIUM:
        print("This action has moderate risk:")
        print("  • Should be reviewed before execution")
        print("  • May have unintended side effects")
        print("  • Consider backing up important data\n")

    elif risk_level == RiskLevel.LOW:
        print("This action appears safe for most users.\n")

    # Offer alternative approaches
    if risk_level in (RiskLevel.HIGH, RiskLevel.FORBIDDEN):
        print("Suggested alternatives:")
        print("  • Consult the documentation")
        print("  • Ask for help in the community")
        print("  • Start with a test on non-critical data\n")

    print("=" * 60)


def check_command_safety(command: str) -> bool:
    """Check if a shell command is safe. Returns True if safe to execute."""
    is_dangerous, pattern, description = detect_dangerous_command(command)

    if not is_dangerous:
        return True

    print(f"\n{Alert(f'Detected potentially dangerous pattern: {pattern}', 'warning').render()}\n")
    print(f"Description: {description}\n")

    # Prompt for approval
    response = prompt_dangerous_approval(command, description)
    return response == "approve"


if __name__ == "__main__":
    security_wizard()
