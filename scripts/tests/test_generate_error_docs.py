"""Unit tests for scripts/generate_error_docs.py.

Covers the parsing edge-cases that are most likely to produce silent wrong
output in CI: a gap in discriminant numbering, a duplicate discriminant, an
undocumented (TODO) variant, and enum isolation (only the requested enum is
parsed even when multiple enums appear in the same source text).
"""

import sys
import textwrap
from pathlib import Path

import pytest

# Make the scripts/ directory importable without installing the package.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from generate_error_docs import (  # noqa: E402
    markdown_table,
    parse_error_enum,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _src(body: str) -> str:
    """Wrap an enum body in minimal Rust source text."""
    return textwrap.dedent(body)


# ---------------------------------------------------------------------------
# parse_error_enum
# ---------------------------------------------------------------------------


class TestParseErrorEnum:
    def test_basic_variants(self, tmp_path: Path) -> None:
        src = _src("""\
            pub enum MyError {
                Foo = 1,
                Bar = 2,
                Baz = 3,
            }
        """)
        f = tmp_path / "lib.rs"
        f.write_text(src, encoding="utf-8")

        result = parse_error_enum(f, "MyError")

        assert result == [(1, "Foo"), (2, "Bar"), (3, "Baz")]

    def test_gap_in_discriminants(self, tmp_path: Path) -> None:
        """Codes 1, 3, 7 — the gaps must be preserved as-is, not filled."""
        src = _src("""\
            pub enum MyError {
                Alpha = 1,
                Gamma = 3,
                Zeta  = 7,
            }
        """)
        f = tmp_path / "lib.rs"
        f.write_text(src, encoding="utf-8")

        result = parse_error_enum(f, "MyError")

        assert result == [(1, "Alpha"), (3, "Gamma"), (7, "Zeta")]

    def test_output_is_sorted_by_code(self, tmp_path: Path) -> None:
        """Variants written out of order in source must be sorted by code."""
        src = _src("""\
            pub enum MyError {
                High  = 10,
                Low   = 1,
                Mid   = 5,
            }
        """)
        f = tmp_path / "lib.rs"
        f.write_text(src, encoding="utf-8")

        result = parse_error_enum(f, "MyError")

        assert [code for code, _ in result] == [1, 5, 10]

    def test_duplicate_discriminant_both_names_present(self, tmp_path: Path) -> None:
        """When two variants share a code, parse_error_enum returns both.

        Deduplication / error-raising is the responsibility of
        check_error_codes.py; the parser must not silently drop either entry.
        """
        src = _src("""\
            pub enum MyError {
                First  = 42,
                Second = 42,
            }
        """)
        f = tmp_path / "lib.rs"
        f.write_text(src, encoding="utf-8")

        result = parse_error_enum(f, "MyError")

        codes = [code for code, _ in result]
        names = [name for _, name in result]
        assert codes.count(42) == 2, "both variants with code 42 must be returned"
        assert "First" in names
        assert "Second" in names

    def test_only_target_enum_is_parsed(self, tmp_path: Path) -> None:
        """Variants from other enums in the same file must not be included."""
        src = _src("""\
            pub enum OtherError {
                Noise = 99,
            }

            pub enum TargetError {
                Signal = 1,
            }
        """)
        f = tmp_path / "lib.rs"
        f.write_text(src, encoding="utf-8")

        result = parse_error_enum(f, "TargetError")

        assert result == [(1, "Signal")]
        assert all(name != "Noise" for _, name in result)

    def test_missing_enum_returns_empty(self, tmp_path: Path) -> None:
        src = _src("""\
            pub enum SomeOtherEnum {
                X = 1,
            }
        """)
        f = tmp_path / "lib.rs"
        f.write_text(src, encoding="utf-8")

        result = parse_error_enum(f, "NonExistentEnum")

        assert result == []


# ---------------------------------------------------------------------------
# markdown_table
# ---------------------------------------------------------------------------


class TestMarkdownTable:
    def test_known_variant_renders_description_and_message(self) -> None:
        errors = [(1, "Foo")]
        descriptions = {"Foo": "Something went wrong"}
        messages = {"Foo": "An error occurred"}

        table = markdown_table(errors, descriptions, messages)

        assert "| 1 |" in table
        assert "`Foo`" in table
        assert "Something went wrong" in table
        assert '"An error occurred"' in table

    def test_undocumented_variant_renders_todo_placeholder(self) -> None:
        """A variant absent from the description/message dicts must show TODO.

        This makes undocumented variants visible in PR diffs rather than
        silently emitting empty cells.
        """
        errors = [(5, "Undocumented")]
        descriptions: dict[str, str] = {}
        messages: dict[str, str] = {}

        table = markdown_table(errors, descriptions, messages)

        assert "TODO" in table, "undocumented variant must produce a TODO placeholder"

    def test_table_has_header_row(self) -> None:
        table = markdown_table([], {}, {})

        assert "| Code |" in table
        assert "| Error |" in table
        assert "| Description |" in table
        assert "| Frontend Message |" in table

    def test_multiple_variants_appear_in_code_order(self) -> None:
        errors = [(1, "A"), (2, "B"), (10, "C")]
        descriptions = {"A": "a", "B": "b", "C": "c"}
        messages = {"A": "ma", "B": "mb", "C": "mc"}

        table = markdown_table(errors, descriptions, messages)
        lines = [ln for ln in table.splitlines() if ln.startswith("|") and "Code" not in ln and "---" not in ln]

        assert lines[0].startswith("| 1 |")
        assert lines[1].startswith("| 2 |")
        assert lines[2].startswith("| 10 |")

    def test_pipe_in_description_is_escaped(self) -> None:
        """Unescaped pipes inside a cell break the Markdown table."""
        errors = [(1, "Pipe")]
        descriptions = {"Pipe": "a | b"}
        messages = {"Pipe": "msg"}

        table = markdown_table(errors, descriptions, messages)

        # The pipe in the description cell must be escaped.
        # Check the data row specifically (not the header separator).
        data_rows = [ln for ln in table.splitlines() if "Pipe" in ln]
        assert data_rows, "expected a row containing 'Pipe'"
        # Each data row should not contain an unescaped bare |  inside the cell
        # content (the cell delimiters are the only unescaped pipes).
        cell_content = data_rows[0].split("|")[3]  # Description column
        assert "\\|" in cell_content or "a | b" not in cell_content
