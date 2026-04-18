"""Harvey Terminal Engine (HTE) — Universal rendering, widgets, and wizard framework.

Core modules:
- rendering_standards: Cross-CLI capability detection and color/unicode constants
- renderer: Safe terminal output with capability checking
- widgets: UI primitives (progress bar, panel, badge, spinner, menu, etc.)
- animation: Terminal animations (spinner, typewriter, fade, loading wave, etc.)
- wizard: Universal multi-step wizard framework
- menu: Interactive menu system with arrow-key navigation
"""

from .rendering_standards import (
    TERM_WIDTH,
    TERM_HEIGHT,
    IS_TTY,
    SUPPORTS_COLOR,
    SUPPORTS_UNICODE,
    SUPPORTS_ANIMATION,
    COLOR_TIER,
    COLORS,
    BOX,
    SYMBOLS,
    STATUS_COLORS,
)

from .renderer import Renderer

from .widgets import (
    ProgressBar,
    Panel,
    StatusBadge,
    Spinner,
    SpeechBubble,
    Table,
    Column,
    Header,
    TextBlock,
    Menu as MenuWidget,
    StatCard,
    Alert,
)

from .animation import (
    SpinnerAnimation,
    TypewriterEffect,
    FadeIn,
    ProgressAnimation,
    LoadingWave,
    PulseAnimation,
)

from .wizard import Wizard, WizardStep

from .menu import Menu, MenuItem, MenuCategory, DynamicMenu

__all__ = [
    # Constants
    "TERM_WIDTH",
    "TERM_HEIGHT",
    "IS_TTY",
    "SUPPORTS_COLOR",
    "SUPPORTS_UNICODE",
    "SUPPORTS_ANIMATION",
    "COLOR_TIER",
    "COLORS",
    "BOX",
    "SYMBOLS",
    "STATUS_COLORS",
    # Classes
    "Renderer",
    "ProgressBar",
    "Panel",
    "StatusBadge",
    "Spinner",
    "SpeechBubble",
    "Table",
    "Column",
    "Header",
    "TextBlock",
    "MenuWidget",
    "StatCard",
    "Alert",
    "SpinnerAnimation",
    "TypewriterEffect",
    "FadeIn",
    "ProgressAnimation",
    "LoadingWave",
    "PulseAnimation",
    "Wizard",
    "WizardStep",
    "Menu",
    "MenuItem",
    "MenuCategory",
    "DynamicMenu",
]
