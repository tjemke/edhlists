"""Argument parsing and the top-level error handling."""
from __future__ import annotations

import pytest

from edh_scout.cli import build_parser, main
from edh_scout.fetch import Http, OfflineMiss


@pytest.fixture
def parser():
    return build_parser()


# --- defaults ------------------------------------------------------------------------------------

def test_scout_defaults(parser):
    args = parser.parse_args(["scout", "Atraxa, Praetors' Voice"])
    assert args.cmd == "scout"
    assert args.commander == "Atraxa, Praetors' Voice"
    assert args.n == 10
    assert args.bracket == 3
    assert args.sample == "recent"
    assert args.offline is False
    assert args.spellbook is False


def test_power_level_is_on_by_default(parser):
    for argv in (["scout", "X"], ["check", "deck.txt"]):
        assert parser.parse_args(argv).power_level is True


def test_no_power_level_switches_it_off(parser):
    for argv in (["scout", "X", "--no-power-level"], ["check", "deck.txt", "--no-power-level"]):
        assert parser.parse_args(argv).power_level is False


def test_check_accepts_repeated_commanders(parser):
    args = parser.parse_args(
        ["check", "deck.txt", "--commander", "Thrasios, Triton Hero", "--commander", "Tymna the Weaver"])
    assert args.commander == ["Thrasios, Triton Hero", "Tymna the Weaver"]


def test_declared_bracket_is_parsed_into_a_set(parser):
    assert parser.parse_args(["scout", "X", "--declared-bracket", "1-3"]).declared_bracket == {1, 2, 3}


def test_bad_declared_bracket_is_rejected(parser):
    with pytest.raises(SystemExit):
        parser.parse_args(["scout", "X", "--declared-bracket", "9"])


def test_bracket_choices_are_enforced(parser):
    with pytest.raises(SystemExit):
        parser.parse_args(["check", "deck.txt", "--bracket", "7"])


def test_subcommand_is_required(parser):
    with pytest.raises(SystemExit):
        parser.parse_args([])


def test_sync_flags(parser):
    args = parser.parse_args(["sync", "--skip-cards", "--skip-combos"])
    assert args.skip_cards and args.skip_combos


def test_offline_and_user_agent_are_shared_by_every_subcommand(parser):
    for argv in (["sync"], ["scout", "X"], ["check", "d.txt"]):
        args = parser.parse_args(argv + ["--offline", "--user-agent", "custom/1.0"])
        assert args.offline is True
        assert args.user_agent == "custom/1.0"


# --- error handling ------------------------------------------------------------------------------

def test_offline_miss_exits_with_a_useful_message(monkeypatch, capsys):
    def boom(args, http):
        raise OfflineMiss("https://example.invalid/thing.json")

    monkeypatch.setattr("edh_scout.cli.cmd_check", boom)
    with pytest.raises(SystemExit) as exc:
        main(["check", "deck.txt", "--offline"])
    assert "not cached" in str(exc.value)
    assert "example.invalid" in str(exc.value)


def test_keyboard_interrupt_exits_130(monkeypatch):
    def boom(args, http):
        raise KeyboardInterrupt

    monkeypatch.setattr("edh_scout.cli.cmd_check", boom)
    with pytest.raises(SystemExit) as exc:
        main(["check", "deck.txt"])
    assert exc.value.code == 130


def test_offline_http_never_opens_a_connection():
    with pytest.raises(OfflineMiss):
        Http(offline=True).get("https://example.invalid/")
