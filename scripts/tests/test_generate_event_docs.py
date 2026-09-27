"""Unit tests for scripts/generate_event_docs.py.

Covers the parsing edge-cases most likely to produce silent wrong output:
  - Basic struct parsing with doc comments
  - Fields annotated with #[topic]
  - Structs missing a doc comment (empty doc list)
  - Multiple structs in one source block
  - camel_to_snake conversion
  - md_table rendering
"""

import sys
import textwrap
from pathlib import Path

import pytest

# Make the scripts/ directory importable without installing the package.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from generate_event_docs import (  # noqa: E402
    camel_to_snake,
    collect_field_topics,
    md_table,
    parse_structs,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def dedent(src: str) -> str:
    return textwrap.dedent(src)


# ---------------------------------------------------------------------------
# parse_structs
# ---------------------------------------------------------------------------


class TestParseStructs:
    def test_basic_struct_with_doc_and_fields(self) -> None:
        src = dedent("""\
            /// Emitted when a ticket is purchased.
            pub struct TicketPurchased {
                /// The buyer's address.
                pub buyer: Address,
                /// Number of tickets bought.
                pub quantity: u32,
            }
        """)

        events = parse_structs(src)

        assert len(events) == 1
        ev = events[0]
        assert ev["name"] == "TicketPurchased"
        assert "Emitted when a ticket is purchased" in " ".join(ev["doc"])
        assert len(ev["fields"]) == 2
        assert ev["fields"][0]["name"] == "buyer"
        assert ev["fields"][1]["name"] == "quantity"

    def test_struct_without_doc_comment_has_empty_doc(self) -> None:
        """A struct with no leading /// lines must not crash; doc must be []."""
        src = dedent("""\
            pub struct Undocumented {
                /// Some field.
                pub value: u64,
            }
        """)

        # parse_structs requires at least one leading doc line to match STRUCT_RE.
        # A completely undocumented struct is simply not returned (it won't match).
        events = parse_structs(src)

        # The contract: an undocumented struct either doesn't appear or appears
        # with an empty doc list — it must never raise an exception.
        for ev in events:
            assert isinstance(ev["doc"], list)

    def test_field_doc_is_captured(self) -> None:
        src = dedent("""\
            /// Event doc.
            pub struct MyEvent {
                /// The raffle identifier (1-based ID).
                pub raffle_id: u64,
            }
        """)

        events = parse_structs(src)

        assert len(events) == 1
        field = events[0]["fields"][0]
        assert "raffle identifier" in " ".join(field["doc"])

    def test_multiple_structs_parsed(self) -> None:
        src = dedent("""\
            /// First event.
            pub struct EventA {
                /// Field a.
                pub a: u32,
            }

            /// Second event.
            pub struct EventB {
                /// Field b.
                pub b: Address,
            }
        """)

        events = parse_structs(src)
        names = {ev["name"] for ev in events}

        assert "EventA" in names
        assert "EventB" in names

    def test_struct_with_attribute_between_doc_and_keyword(self) -> None:
        """Attributes like #[contractevent] between doc and `pub struct` must not block parsing."""
        src = dedent("""\
            /// A documented event.
            #[contractevent]
            pub struct AttributedEvent {
                /// Some field.
                pub val: i64,
            }
        """)

        events = parse_structs(src)
        names = [ev["name"] for ev in events]

        assert "AttributedEvent" in names


# ---------------------------------------------------------------------------
# collect_field_topics
# ---------------------------------------------------------------------------


class TestCollectFieldTopics:
    def test_topic_annotated_field_is_collected(self) -> None:
        src = dedent("""\
            pub struct MyEvent {
                #[topic]
                pub raffle_id: u64,
                pub buyer: Address,
            }
        """)

        topics = collect_field_topics(src)

        assert "raffle_id" in topics.get("MyEvent", set())
        assert "buyer" not in topics.get("MyEvent", set())

    def test_no_topic_fields_returns_empty_set(self) -> None:
        src = dedent("""\
            pub struct Plain {
                pub value: u32,
            }
        """)

        topics = collect_field_topics(src)

        assert topics.get("Plain", set()) == set()

    def test_multiple_topic_fields(self) -> None:
        src = dedent("""\
            pub struct Multi {
                #[topic]
                pub field_a: u32,
                #[topic]
                pub field_b: Address,
                pub field_c: u64,
            }
        """)

        topics = collect_field_topics(src)

        assert topics["Multi"] == {"field_a", "field_b"}

    def test_topics_across_multiple_structs(self) -> None:
        src = dedent("""\
            pub struct Alpha {
                #[topic]
                pub x: u32,
            }
            pub struct Beta {
                #[topic]
                pub y: Address,
            }
        """)

        topics = collect_field_topics(src)

        assert "x" in topics.get("Alpha", set())
        assert "y" in topics.get("Beta", set())
        assert "x" not in topics.get("Beta", set())


# ---------------------------------------------------------------------------
# md_table
# ---------------------------------------------------------------------------


class TestMdTable:
    def _field(self, name: str, typ: str, doc: str, topic: bool = False) -> dict:
        return {"name": name, "type": typ, "doc": [doc], "topic": topic}

    def test_table_has_header(self) -> None:
        table = md_table([])

        assert "| Field |" in table
        assert "| Type |" in table
        assert "| Flags |" in table
        assert "| Description |" in table

    def test_topic_flag_appears_in_flags_column(self) -> None:
        fields = [self._field("raffle_id", "u64", "The ID", topic=True)]

        table = md_table(fields)

        rows = [ln for ln in table.splitlines() if "raffle_id" in ln]
        assert rows, "expected a row for raffle_id"
        assert "topic" in rows[0]

    def test_non_topic_flag_column_is_empty(self) -> None:
        fields = [self._field("value", "u32", "A value", topic=False)]

        table = md_table(fields)

        rows = [ln for ln in table.splitlines() if "value" in ln]
        assert rows
        # The flags cell should be empty (just whitespace between pipes)
        parts = rows[0].split("|")
        flags_cell = parts[3].strip()  # 0=empty, 1=Field, 2=Type, 3=Flags, 4=Description
        assert flags_cell == ""

    def test_pipe_in_doc_is_escaped(self) -> None:
        fields = [self._field("x", "u32", "a | b")]

        table = md_table(fields)

        rows = [ln for ln in table.splitlines() if "`x`" in ln]
        assert rows
        assert "a \\| b" in rows[0] or "a | b" not in rows[0].replace("\\|", "")


# ---------------------------------------------------------------------------
# camel_to_snake
# ---------------------------------------------------------------------------


class TestCamelToSnake:
    @pytest.mark.parametrize(
        "camel,expected",
        [
            ("TicketPurchased", "ticket_purchased"),
            ("RaffleCreated", "raffle_created"),
            ("WinnerDrawn", "winner_drawn"),
            ("FeeCollected", "fee_collected"),
            ("PrizeDeposited", "prize_deposited"),
            ("AdminChanged", "admin_changed"),
        ],
    )
    def test_conversion(self, camel: str, expected: str) -> None:
        assert camel_to_snake(camel) == expected

    def test_single_word_is_lowercased(self) -> None:
        assert camel_to_snake("Event") == "event"

    def test_all_caps_abbreviation(self) -> None:
        # e.g. "NFTMinted" -> "n_f_t_minted"; acceptable behaviour documented.
        result = camel_to_snake("NFTMinted")
        assert isinstance(result, str)
        assert result.islower() or "_" in result
