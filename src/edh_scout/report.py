"""Rendering results: a terse table on stdout, and a linkable report.md next to the decklists."""
from __future__ import annotations

import time
from pathlib import Path

from .config import VERSION
from .deck import Deck
from .evaluate import Result
from .powerlevel import edhpowerlevel_url


def emit(results: list[tuple[Deck, Result]], target: int, out: Path, title: str, only_fit: bool) -> None:
    (out / "decks").mkdir(parents=True, exist_ok=True)
    results = sorted(results, key=lambda dr: (
        dr[1].floor > target, not dr[1].hint(target), dr[1].floor,
        len(dr[1].game_changers), dr[0].meta.get("price") or 0))
    md = [f"# {title}: bracket triage (target B{target})", "",
          f"Generated {time.strftime('%Y-%m-%d %H:%M')} by edh_scout {VERSION}. A floor is the *minimum* "
          "bracket implied by Game Changers, mass land denial, extra turns and known combos; it can't "
          "see speed, synergy or pilot intent. Power level is [EDHPowerLevel]"
          "(https://edhpowerlevel.com)'s 1-10 scale, scored locally from card price and EDHREC rank -- "
          "follow a deck's link to check it against the site. "
          "Combo data: [Commander Spellbook](https://commanderspellbook.com).",
          ""]
    print(f"\n{'':1} {'B>=':>3} {'PL':>4} {'GC':>2} {'cmb':>3} {'$':>6} {'salt':>5} "
          f"{'MV':>4} {'tut':>3}  deck")
    shown = 0
    for deck, r in results:
        fit, hint = r.floor <= target, r.hint(target)
        if only_fit and not fit:
            continue
        shown += 1
        (out / "decks" / f"{deck.ident}.txt").write_text("\n".join(deck.lines()) + "\n", encoding="utf-8")
        mark = "✓" if fit else "~" if hint else "✗"
        floor = "ban" if r.floor >= 99 else str(r.floor)
        st, m = r.stats, deck.meta
        price = f"{m['price']:.0f}" if isinstance(m.get("price"), (int, float)) else "-"
        salt = f"{m['salt']:.1f}" if isinstance(m.get("salt"), (int, float)) else "-"
        pl = r.power.label() if r.power else "-"
        print(f"{mark} {floor:>3} {pl:>4} {len(r.game_changers):>2} {len(r.combos):>3} {price:>6} "
              f"{salt:>5} {st.get('avg_mv', '-'):>4} {st.get('tutors', '-'):>3}  {deck.ident}")
        if r.power:
            print(f"      PL {r.power.label()}/10: impact {r.power.impact:.0f} × efficiency "
                  f"{r.power.efficiency:.2f} = score {r.power.score:.0f}")
        for line in ([hint] if hint else []) + r.reasons():
            print(f"      {line}")

        links = [f"[EDHPowerLevel]({edhpowerlevel_url(deck.lines())})"]
        if deck.edhrec:
            links.insert(0, f"[EDHREC]({deck.edhrec})")
        if deck.source:
            links.insert(1, f"[source]({deck.source})")
        md += [f"## {mark} B≥{floor} · `{deck.ident}`", "",
               "- " + " · ".join(links) + f" · list: `decks/{deck.ident}.txt`",
               f"- ${price} · salt {salt} · {st.get('cards', '?')} cards · "
               f"{st.get('lands', '?')} lands · avg MV {st.get('avg_mv', '?')} · "
               f"tutors {st.get('tutors', '?')} · cheap ramp {st.get('ramp', '?')}",
               "- engine: " + ("Commander Spellbook /estimate-bracket" if r.engine == "spellbook"
                               else "offline DB + heuristics")]
        if r.power:
            p = r.power
            md.append(f"- **power level {p.label()}/10** · score {p.score:.0f} "
                      f"(impact {p.impact:.0f} × efficiency {p.efficiency:.2f}) · "
                      f"avg MV {p.avg_cmc} · 65% of impact by MV {p.curve_cmc}")
        md += [f"- **{hint}**"] if hint else []
        md += [f"- {line}" for line in r.reasons()]
        if deck.meta.get("tags"):
            md.append("- tags: " + ", ".join(map(str, deck.meta["tags"])))
        if r.unknown:
            md.append(f"- unrecognised names ({len(r.unknown)}): " + ", ".join(r.unknown[:10]))
        if st and st.get("cards", 100) < 95:
            md.append(f"- ⚠ only {st['cards']} cards in the EDHREC snapshot; list may be incomplete")
        md.append("")
    (out / "report.md").write_text("\n".join(md), encoding="utf-8")
    print(f"\n{shown} deck(s) -> {out}/report.md  (✓ fits B{target}, ~ fixable, ✗ above)")
