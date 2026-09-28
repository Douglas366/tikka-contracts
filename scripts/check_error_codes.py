#!/usr/bin/env python3
"""CI check: ensure no duplicate or reused discriminants within each Rust error enum.

Usage:
    python scripts/check_error_codes.py

Exits non-zero if a duplicate discriminant is found within any single enum.
"""

import re
import sys
from pathlib import Path


def parse_error_enum(file_path: Path, enum_name: str) -> list[tuple[int, str]]:
    content = file_path.read_text(encoding="utf-8")

    enum_match = re.search(
        r"(?:#\[contracterror\].*?)?pub enum " + enum_name + r" \{(.*?)\}",
        content,
        re.DOTALL,
    )

    if not enum_match:
        return []

    enum_body = enum_match.group(1)
    errors = []
    for match in re.finditer(r"(\w+)\s*=\s*(\d+)", enum_body):
        errors.append((int(match.group(2)), match.group(1)))

    return errors


def check_duplicates(errors: list[tuple[int, str]], enum_name: str) -> bool:
    seen: dict[int, str] = {}
    for code, name in errors:
        if code in seen:
            print(
                f"ERROR: Duplicate discriminant {code} in {enum_name}: "
                f"{seen[code]} and {name}"
            )
            return False
        seen[code] = name
    return True


def main() -> None:
    repo_root = Path(__file__).parent.parent
    instance_file = repo_root / "contracts" / "raffle-instance" / "src" / "lib.rs"
    factory_file = repo_root / "contracts" / "raffle-factory" / "src" / "lib.rs"

    instance_errors = parse_error_enum(instance_file, "Error")
    factory_errors = parse_error_enum(factory_file, "ContractError")

    ok = check_duplicates(instance_errors, "raffle-instance::Error")
    ok = check_duplicates(factory_errors, "raffle-factory::ContractError") and ok

    if not ok:
        print("\nCI check FAILED: duplicate discriminants found.")
        sys.exit(1)

    print("CI check PASSED: no duplicate discriminants.")


if __name__ == "__main__":
    main()
