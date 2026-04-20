"""Universal wizard framework — interactive step-by-step guided flows."""

import sys
from dataclasses import dataclass, field
from typing import Any, Callable, List, Dict, Optional, Union, Tuple
from . import rendering_standards as rs
from .widgets import Panel, StatusBadge, SpeechBubble
from .animation import SpinnerAnimation


@dataclass
class WizardStep:
    """A single step in a wizard."""

    id: str
    title: str  # plain english title shown at top
    prompt: str  # what the user sees
    help_text: str  # shown on '?' press — no jargon
    input_type: str  # "text" | "select" | "confirm" | "multi_select" | "nl_prompt" | "password"
    choices: List[str] = field(default_factory=list)  # for select/multi_select types
    validate: Optional[Callable[[str], Tuple[bool, str]]] = None  # returns (is_valid, error_msg)
    default: Any = None  # pre-filled value
    skip_if: Optional[Callable[[Dict[str, Any]], bool]] = None  # skip this step if condition true


class Wizard:
    """Orchestrates a multi-step wizard with arrow-key navigation."""

    def __init__(self, title: str, steps: List[WizardStep], subtitle: str = ""):
        self.title = title
        self.subtitle = subtitle
        self.steps = steps
        self.current_step_index = 0
        self.results = {}
        self.completed = False

    def _show_header(self):
        """Show wizard title and progress."""
        print(f"\n{rs.COLORS.get('bold', '')}{self.title}{rs.COLORS.get('reset', '')}")
        if self.subtitle:
            print(f"{rs.COLORS.get('dim', '')}{self.subtitle}{rs.COLORS.get('reset', '')}\n")

        # Progress indicator
        total = len(self.steps)
        current = self.current_step_index + 1
        progress = f"Step {current}/{total}"
        print(f"{rs.COLORS.get('cyan', '')}{progress}{rs.COLORS.get('reset', '')}\n")

    def _handle_text_input(self, step: WizardStep) -> str:
        """Handle text input."""
        default_str = f" [{step.default}]" if step.default else ""
        prompt_text = f"{step.prompt}{default_str}: "

        while True:
            user_input = input(prompt_text).strip()

            # Use default if empty
            if not user_input and step.default:
                user_input = str(step.default)

            # Validate
            if step.validate:
                is_valid, error_msg = step.validate(user_input)
                if not is_valid:
                    print(f"{rs.COLORS.get('red', '')}Error: {error_msg}{rs.COLORS.get('reset', '')}")
                    continue

            return user_input

    def _handle_select_input(self, step: WizardStep) -> str:
        """Handle arrow-key menu selection."""
        selected = 0
        if step.default and step.default in step.choices:
            selected = step.choices.index(step.default)

        while True:
            print(f"\n{step.prompt} (use arrow keys, Enter to select):\n")

            for i, choice in enumerate(step.choices):
                marker = "► " if i == selected else "  "
                color = rs.COLORS.get("green", "") if i == selected else ""
                print(f"{color}{marker}{choice}{rs.COLORS.get('reset', '')}")

            print(f"\n(↑↓ navigate, Enter select, ? help)")

            # Simple stdin handling — in real TTY we'd use termios
            # For now, use number-based selection as fallback
            try:
                user_input = input("Selection (0-9 or Enter): ").strip()

                if user_input == "" or user_input == "\r":
                    return step.choices[selected]

                if user_input == "?":
                    print(f"\n{step.help_text}\n")
                    continue

                try:
                    idx = int(user_input)
                    if 0 <= idx < len(step.choices):
                        return step.choices[idx]
                except ValueError:
                    pass

            except KeyboardInterrupt:
                raise

    def _handle_confirm_input(self, step: WizardStep) -> bool:
        """Handle yes/no confirmation."""
        default_char = "Y" if step.default else "n"
        prompt_text = f"{step.prompt} [{default_char}/{'n' if default_char == 'Y' else 'Y'}]: "

        while True:
            user_input = input(prompt_text).strip().lower()

            if not user_input:
                return step.default if step.default is not None else True

            if user_input in ("y", "yes"):
                return True
            if user_input in ("n", "no"):
                return False

            print(f"{rs.COLORS.get('red', '')}Please enter 'y' or 'n'{rs.COLORS.get('reset', '')}")

    def _handle_multi_select_input(self, step: WizardStep) -> List[str]:
        """Handle multi-select menu."""
        selected = set()
        if step.default:
            for item in (step.default if isinstance(step.default, list) else [step.default]):
                if item in step.choices:
                    selected.add(step.choices.index(item))

        while True:
            print(f"\n{step.prompt} (space to toggle, Enter when done):\n")

            for i, choice in enumerate(step.choices):
                marker = "[x]" if i in selected else "[ ]"
                color = rs.COLORS.get("green", "") if i in selected else ""
                print(f"{color}{marker} {choice}{rs.COLORS.get('reset', '')}")

            print(f"\n(Space to toggle, Enter to continue, ? help)")

            try:
                user_input = input("Selection: ").strip()

                if user_input == "":
                    result = [step.choices[i] for i in sorted(selected)]
                    if result:
                        return result
                    print(f"{rs.COLORS.get('red', '')}Please select at least one item{rs.COLORS.get('reset', '')}")
                    continue

                if user_input == "?":
                    print(f"\n{step.help_text}\n")
                    continue

                try:
                    idx = int(user_input)
                    if 0 <= idx < len(step.choices):
                        if idx in selected:
                            selected.remove(idx)
                        else:
                            selected.add(idx)
                except ValueError:
                    pass

            except KeyboardInterrupt:
                raise

    def _handle_nl_prompt(self, step: WizardStep) -> str:
        """Handle natural language prompt."""
        print(f"\n{step.prompt}")
        print(f"{rs.COLORS.get('dim', '')}(Describe it in plain English){rs.COLORS.get('reset', '')}\n")

        user_input = input("> ").strip()

        if not user_input:
            print(f"{rs.COLORS.get('red', '')}Please provide a response{rs.COLORS.get('reset', '')}")
            return self._handle_nl_prompt(step)

        return user_input

    def _handle_password_input(self, step: WizardStep) -> str:
        """Handle password input (masked)."""
        import getpass

        prompt_text = f"{step.prompt}: "
        password = getpass.getpass(prompt_text)

        if not password and step.default:
            return str(step.default)

        return password

    def _get_next_step(self) -> Optional[WizardStep]:
        """Get next non-skipped step."""
        while self.current_step_index < len(self.steps):
            step = self.steps[self.current_step_index]

            # Check skip condition
            if step.skip_if and step.skip_if(self.results):
                self.current_step_index += 1
                continue

            return step

        return None

    def run(self) -> Dict[str, Any]:
        """Run the wizard and return results."""
        try:
            while self.current_step_index < len(self.steps):
                step = self._get_next_step()

                if not step:
                    break

                self._show_header()

                # Handle different input types
                if step.input_type == "text":
                    result = self._handle_text_input(step)
                elif step.input_type == "select":
                    result = self._handle_select_input(step)
                elif step.input_type == "confirm":
                    result = self._handle_confirm_input(step)
                elif step.input_type == "multi_select":
                    result = self._handle_multi_select_input(step)
                elif step.input_type == "nl_prompt":
                    result = self._handle_nl_prompt(step)
                elif step.input_type == "password":
                    result = self._handle_password_input(step)
                else:
                    raise ValueError(f"Unknown input type: {step.input_type}")

                self.results[step.id] = result
                self.current_step_index += 1

            self.completed = True
            print(f"\n{rs.COLORS.get('green', '')}✓ Complete!{rs.COLORS.get('reset', '')}\n")
            return self.results

        except KeyboardInterrupt:
            print(f"\n\n{rs.COLORS.get('yellow', '')}Wizard cancelled.{rs.COLORS.get('reset', '')}\n")
            return {}


# ── Demo ────────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    demo_steps = [
        WizardStep(
            id="name",
            title="What's your name?",
            prompt="Enter your name",
            help_text="Your first name is fine.",
            input_type="text",
            default="Harvey",
        ),
        WizardStep(
            id="os",
            title="What operating system?",
            prompt="Choose your OS",
            help_text="This determines which tools are available.",
            input_type="select",
            choices=["macOS", "Linux", "Windows"],
            default="macOS",
        ),
        WizardStep(
            id="features",
            title="Which features?",
            prompt="Enable features",
            help_text="You can change these later.",
            input_type="multi_select",
            choices=["Network", "Security", "Files", "Health"],
            default=["Network", "Health"],
        ),
        WizardStep(
            id="confirm",
            title="Ready?",
            prompt="Proceed with setup",
            help_text="You can always reconfigure later.",
            input_type="confirm",
            default=True,
        ),
    ]

    wizard = Wizard("Harvey Setup Wizard", demo_steps, "Let's configure Harvey OS")
    results = wizard.run()

    print(f"\n{rs.COLORS.get('bold', '')}Results:{rs.COLORS.get('reset', '')}")
    for key, value in results.items():
        print(f"  {key}: {value}")
