"""edh_scout — pull EDHREC decklists for a commander and triage them by Commander bracket.

Scouts EDHREC for decks built around a commander, then scores each one: the minimum Commander
bracket it can legally claim, and EDHPowerLevel's 1-10 power level. See README.md for usage.

Stdlib only at runtime. Cache/DB live in $XDG_CACHE_HOME/edh-scout (default ~/.cache/edh-scout).
"""
from __future__ import annotations

__version__ = "0.1"
__all__ = ["__version__"]
