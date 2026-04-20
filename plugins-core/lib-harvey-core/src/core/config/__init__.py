"""Makakoo OS config package.

`persona` — AI persona config (name, user, pronouns). Sebastian's default
install uses name="Harvey" so everything else in the codebase keeps working
without a single hardcoded prompt change.
"""

from .persona import Persona, load, reload

__all__ = ["Persona", "load", "reload"]
