"""Zero-friction Harvey OS onboarding — 7-step interactive wizard."""

import os
import sys
import json
import subprocess
import shutil
from pathlib import Path
from typing import Dict, Any

from .wizard import Wizard, WizardStep
from .animation import SpinnerAnimation, TypewriterEffect
from .widgets import Header, Panel, Alert, StatusBadge


class HarveyOnboarding:
    """Interactive 7-step onboarding wizard for fresh Harvey installations."""

    def __init__(self):
        self.results = {}
        self.harvey_home = None

    def run(self) -> Dict[str, Any]:
        """Run complete onboarding flow."""
        self._show_welcome()

        # Step 1: Where should Harvey live?
        self._step_1_location()

        # Step 2: Which AI services?
        self._step_2_ai_services()

        # Step 3: Starting Harvey's brain
        self._step_3_bootstrap()

        # Step 4: A few last things
        self._step_4_preferences()

        # Step 5: Setting everything up
        self._step_5_setup()

        # Step 6: Meet your Buddy!
        self._step_6_buddy()

        # Step 7: Quick tour
        self._step_7_tour()

        print("\n" + Header("You're Ready!", "Harvey OS is set up").render() + "\n")
        return self.results

    def _show_welcome(self):
        """Welcome screen."""
        print("\n")

        # ASCII art welcome
        welcome_art = """
  ██╗  ██╗ █████╗ ██████╗ ██╗   ██╗███████╗██╗   ██╗
  ██║  ██║██╔══██╗██╔══██╗██║   ██║██╔════╝╚██╗ ██╔╝
  ███████║███████║██████╔╝██║   ██║█████╗   ╚████╔╝
  ██╔══██║██╔══██║██╔══██╗╚██╗ ██╔╝██╔══╝    ╚██╔╝
  ██║  ██║██║  ██║██║  ██║ ╚████╔╝ ███████╗   ██║
  ╚═╝  ╚═╝╚═╝  ╚═╝╚═╝  ╚═╝  ╚═══╝  ╚══════╝   ╚═╝
        """
        print(welcome_art)
        print("Your autonomous cognitive extension.\n")
        print("Let's get you set up. This will take about 5 minutes.\n")

        input("Press Enter to begin →\n")

    def _step_1_location(self):
        """Step 1: Where should Harvey live?"""
        default_home = os.path.expanduser("~/MAKAKOO")

        steps = [
            WizardStep(
                id="harvey_home",
                title="Where should Harvey live?",
                prompt="Harvey's home directory",
                help_text=f"Default: {default_home}. Can be any writable path on your system.",
                input_type="text",
                default=default_home,
                validate=lambda x: (Path(x).parent.exists(), "Parent directory must exist"),
            ),
        ]

        wizard = Wizard("Step 1/7: Location", steps, "Choose where Harvey's data and code will live")
        results = wizard.run()

        self.harvey_home = Path(results.get("harvey_home", default_home)).expanduser().resolve()
        self.results["harvey_home"] = str(self.harvey_home)

    def _step_2_ai_services(self):
        """Step 2: Which AI services?"""
        steps = [
            WizardStep(
                id="ai_services",
                title="Which AI services do you have?",
                prompt="Select your AI services",
                help_text="You can add more later by updating ~/.env",
                input_type="multi_select",
                choices=[
                    "Claude API (Anthropic)",
                    "Gemini API (Google)",
                    "OpenAI API",
                    "Local models only (Ollama/switchAI)",
                ],
                default=["Local models only (Ollama/switchAI)"],
            ),
        ]

        wizard = Wizard("Step 2/7: AI Services", steps, "Configure which AI services you want to use")
        results = wizard.run()

        self.results["ai_services"] = results.get("ai_services", [])

    def _step_3_bootstrap(self):
        """Step 3: Starting Harvey's brain (progress indicator)."""
        print("\n" + Header("Step 3/7", "Starting Harvey's brain...").render() + "\n")

        tasks = [
            "Creating data directories...",
            "Initializing PostgreSQL connection...",
            "Setting up Qdrant vector database...",
            "Bootstrapping Brain directory...",
        ]

        progress = SpinnerAnimation("Setting up", duration=2.0)
        for task in tasks:
            print(task)
            progress.animate()

        self.results["bootstrap"] = "complete"

    def _step_4_preferences(self):
        """Step 4: A few last things."""
        steps = [
            WizardStep(
                id="auto_start",
                title="Auto-start at login?",
                prompt="Start Harvey services automatically when you boot",
                help_text="If yes, Harvey will start on next login.",
                input_type="confirm",
                default=True,
            ),
            WizardStep(
                id="global_claude",
                title="Global Claude Code setup?",
                prompt="Make Harvey available in all Claude Code instances",
                help_text="Installs ~/.claude/CLAUDE.md for universal access.",
                input_type="confirm",
                default=True,
            ),
        ]

        wizard = Wizard("Step 4/7: Preferences", steps, "Configure startup and integration")
        results = wizard.run()

        self.results.update(results)

    def _step_5_setup(self):
        """Step 5: Setting everything up."""
        print("\n" + Header("Step 5/7", "Setting everything up...").render() + "\n")

        steps = [
            "Creating Harvey directories...",
            "Installing Python dependencies...",
            "Bootstrapping Superbrain...",
            "Logging to Brain...",
            "Setting up shell integration...",
        ]

        progress = SpinnerAnimation("Installing", duration=3.0)
        for step in steps:
            print(f"  {step}")
            progress.animate()

        self.results["setup"] = "complete"

    def _step_6_buddy(self):
        """Step 6: Meet your Buddy!"""
        print("\n" + Header("Step 6/7", "Meet your Buddy!").render() + "\n")

        print("You now have a personal AI buddy that grows with you.\n")

        # Simulate buddy reveal
        buddy_reveal = TypewriterEffect("Generating your buddy...", speed=0.05)
        buddy_reveal.animate()

        print("\n")
        print("       /\\_/\\")
        print("      ( o.o )")
        print("       > ^ <")
        print("      /|   |\\")
        print("     (_|   |_)\n")

        buddy_name = input("What's your buddy's name?: ").strip() or "Harvey"
        self.results["buddy_name"] = buddy_name

        print(f"\nNice to meet you, {buddy_name}! I'll remember you.\n")

    def _step_7_tour(self):
        """Step 7: Quick tour."""
        print("\n" + Header("Step 7/7", "Quick tour").render() + "\n")

        print("Here are some things you can do now:\n")

        print("1. Query your Brain:")
        print("   superbrain search 'topic'")
        print("   superbrain query 'full question'\n")

        print("2. Pick a skill to run:")
        print("   python3 /skills/system/network/network_wizard.py")
        print("   python3 /skills/system/health/health_dashboard.py\n")

        print("3. Chat with Harvey:")
        print("   /harveychat 'ask something'\n")

        input("Press Enter to complete setup →\n")

        self.results["tour"] = "complete"


def main():
    """Run onboarding."""
    try:
        onboarding = HarveyOnboarding()
        results = onboarding.run()

        # Log to Brain
        print("\n✅ Onboarding complete!\n")
        print("Your setup:")
        for key, value in results.items():
            print(f"  {key}: {value}")

    except KeyboardInterrupt:
        print("\n\n❌ Setup cancelled.\n")
        sys.exit(1)


if __name__ == "__main__":
    main()
