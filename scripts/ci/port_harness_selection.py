#!/usr/bin/env python3
"""Choose the ports a pull request's Linux port harness run tests (#1103).

Prints `all`, `none`, or the space-separated `crates/<dir>` names whose
blocks of scripts/test-port-harness.sh run (VESPER_PORT_HARNESS_CRATES).

Everything runs unless every changed file is either documentation (`docs/`
or a top-level `*.md`) or inside a workspace package outside the compiler's
dependency closure. Then the changed packages and every workspace package
that depends on them, transitively and through any dependency kind, run.
So a change to the compiler, the runtime or std packages, a seed copy the
compiler links, the harness script, CI, or the root manifests or lockfile
runs every port, and a change inside one port's crate runs that port and
the ports built on it. Pushes to main, the nightly and the full profile
always run everything.
"""
from __future__ import annotations

import argparse
import glob
import subprocess
import sys
import tomllib
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[2]


def load_workspace(root: Path = ROOT) -> tuple[dict[str, str], dict[str, set[str]], dict[str, set[str]]]:
    """Returns {package: dir}, {package: all workspace deps}, {package: build+normal workspace deps}."""
    manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    workspace = manifest["workspace"]
    dirs: list[str] = []
    for member in workspace["members"]:
        matches = sorted(glob.glob(member, root_dir=root)) if any(c in member for c in "*?[") else [member]
        dirs.extend(PurePosixPath(m).as_posix() for m in matches)
    workspace_paths = {
        name: PurePosixPath(spec["path"]).as_posix()
        for name, spec in workspace.get("dependencies", {}).items()
        if isinstance(spec, dict) and "path" in spec
    }
    by_dir: dict[str, str] = {}
    manifests: dict[str, dict] = {}
    for d in dirs:
        data = tomllib.loads((root / d / "Cargo.toml").read_text(encoding="utf-8"))
        name = data["package"]["name"]
        by_dir[_norm(d)] = name
        manifests[name] = (d, data)
    package_dir = {name: _norm(d) for name, (d, _) in manifests.items()}

    def deps(d: str, data: dict, kinds: tuple[str, ...]) -> set[str]:
        found = set()
        tables = [data]
        tables += [t for t in data.get("target", {}).values() if isinstance(t, dict)]
        for table in tables:
            for kind in kinds:
                for key, spec in table.get(kind, {}).items():
                    if not isinstance(spec, dict):
                        continue
                    if "path" in spec:
                        path = _norm((PurePosixPath(d) / spec["path"]).as_posix())
                    elif spec.get("workspace") and key in workspace_paths:
                        path = _norm(workspace_paths[key])
                    else:
                        continue
                    if path in by_dir:
                        found.add(by_dir[path])
        return found

    every = ("dependencies", "dev-dependencies", "build-dependencies")
    linked = ("dependencies", "build-dependencies")
    all_deps = {n: deps(d, data, every) for n, (d, data) in manifests.items()}
    link_deps = {n: deps(d, data, linked) for n, (d, data) in manifests.items()}
    return package_dir, all_deps, link_deps


def _norm(path: str) -> str:
    parts = []
    for part in PurePosixPath(path).parts:
        if part == "..":
            parts.pop()
        elif part != ".":
            parts.append(part)
    return "/".join(parts)


def closure(start: set[str], edges: dict[str, set[str]]) -> set[str]:
    seen, todo = set(), list(start)
    while todo:
        n = todo.pop()
        if n not in seen:
            seen.add(n)
            todo.extend(edges.get(n, ()))
    return seen


def select(changed: list[str], root: Path = ROOT) -> str:
    package_dir, all_deps, link_deps = load_workspace(root)
    compiler = next(n for n, d in package_dir.items() if d == "")
    compiler_closure = closure({compiler}, link_deps)
    members = sorted(((d, n) for n, d in package_dir.items() if d), key=lambda x: -len(x[0]))
    touched: set[str] = set()
    for file in changed:
        path = PurePosixPath(file)
        if path.parts[:1] == ("docs",) or (len(path.parts) == 1 and path.suffix == ".md"):
            continue
        owner = next((n for d, n in members if file == d or file.startswith(d + "/")), None)
        if owner is None or owner in compiler_closure:
            return "all"
        touched.add(owner)
    if not touched:
        return "none"
    dependents: dict[str, set[str]] = {}
    for n, ds in all_deps.items():
        for d in ds:
            dependents.setdefault(d, set()).add(n)
    selected = closure(touched, dependents)
    crates = {PurePosixPath(package_dir[n]).parts[1] for n in selected
              if PurePosixPath(package_dir[n]).parts[:1] == ("crates",)}
    return " ".join(sorted(crates)) or "none"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--profile", required=True, choices=["pr", "full"])
    parser.add_argument("--event", default="pull_request")
    parser.add_argument("--base")
    parser.add_argument("--head", required=True)
    options = parser.parse_args(argv)
    if options.profile == "full" or options.event != "pull_request" or not options.base:
        print("all")
        return 0
    try:
        out = subprocess.run(
            ["git", "diff", "--name-only", "--no-renames", f"{options.base}...{options.head}"],
            cwd=ROOT, check=True, capture_output=True, text=True,
        ).stdout
    except subprocess.CalledProcessError:
        print("all")
        return 0
    changed = [line for line in out.splitlines() if line]
    print(select(changed) if changed else "all")
    return 0


if __name__ == "__main__":
    sys.exit(main())
