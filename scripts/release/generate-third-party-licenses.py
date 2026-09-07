#!/usr/bin/env python3
"""Generate the runtime Rust dependency license inventory."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess


PROJECT_ROOT = Path(__file__).resolve().parents[2]
OUTPUT = PROJECT_ROOT / "THIRD-PARTY-LICENSES.md"


def runtime_packages() -> list[dict[str, object]]:
    completed = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1"],
        cwd=PROJECT_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    metadata = json.loads(completed.stdout)
    root = metadata["resolve"]["root"]
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    reachable = {root}
    pending = [root]

    while pending:
        package_id = pending.pop()
        for dependency in nodes[package_id].get("deps", []):
            kinds = dependency.get("dep_kinds", [])
            if not any(kind.get("kind") != "dev" for kind in kinds):
                continue
            dependency_id = dependency["pkg"]
            if dependency_id not in reachable:
                reachable.add(dependency_id)
                pending.append(dependency_id)

    return sorted(
        (packages[package_id] for package_id in reachable if package_id != root),
        key=lambda package: (str(package["name"]).lower(), str(package["version"])),
    )


def cell(value: object) -> str:
    return str(value or "Not declared").replace("|", "\\|").replace("\n", " ")


def render() -> str:
    rows = []
    for package in runtime_packages():
        authors = ", ".join(str(author) for author in package.get("authors", []))
        location = package.get("repository") or package.get("homepage") or package.get("source")
        rows.append(
            f"| {cell(package['name'])} | {cell(package['version'])} | "
            f"{cell(package.get('license'))} | {cell(authors)} | {cell(location)} |"
        )

    return "\n".join(
        [
            "# Third-party licenses",
            "",
            "CukeDedup's native binary contains the following non-development Rust dependencies.",
            "The declared SPDX expressions and upstream locations are generated from the locked",
            "Cargo dependency graph. Each dependency remains copyright its respective authors and",
            "is distributed under its declared license terms.",
            "",
            "The complete corresponding source and license files are available from the linked",
            "upstream projects and from each crate's source package on crates.io. This inventory is",
            "regenerated whenever `Cargo.lock` changes.",
            "",
            "| Package | Version | Declared license | Authors | Upstream |",
            "| --- | --- | --- | --- | --- |",
            *rows,
            "",
        ]
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    content = render()
    if args.check:
        if not OUTPUT.exists() or OUTPUT.read_text(encoding="utf-8") != content:
            raise SystemExit(
                "THIRD-PARTY-LICENSES.md is stale; run "
                "python3 scripts/release/generate-third-party-licenses.py"
            )
        return
    OUTPUT.write_text(content, encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
