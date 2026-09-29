"""Paths, service endpoints and the handful of constants shared across the package."""
from __future__ import annotations

import os
import sys
from pathlib import Path

from . import __version__

VERSION = __version__
UA = f"edh-scout/{VERSION} (personal deck-testing script)"
CACHE = Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache") / "edh-scout"
DB_CARDS = CACHE / "cards.json.gz"
DB_COMBOS = CACHE / "combos.json.gz"
# Bumped when a card record gains fields; an older DB still loads, minus the features that need them.
CARDS_SCHEMA = 2

EDHREC_JSON = "https://json.edhrec.com/pages"
EDHREC_SITE = "https://edhrec.com"
SCRYFALL = "https://api.scryfall.com"
SPELLBOOK_API = "https://backend.commanderspellbook.com"
SPELLBOOK_DUMP = "https://json.commanderspellbook.com/variants.json.gz"

# Spellbook bracket tags -> bracket number (the mapping Spellbook itself uses).
TAG_BRACKET = {"E": 1, "C": 2, "O": 2, "P": 3, "S": 3, "R": 4, "B": 99}
TAG_NAME = {"E": "Exhibition", "C": "Core", "O": "Oddball", "P": "Powerful",
            "S": "Spicy", "R": "Ruthless", "B": "Banned"}


def log(msg: str) -> None:
    print(msg, file=sys.stderr)
