#!/usr/bin/env python3
"""Reproduce licenses from locked Cargo archives and installed npm production packages.

Run after `cargo fetch --locked` and `npm ci`. No license grants or copyright
holders are inferred from SPDX identifiers. Missing upstream files are recorded.
The full Cargo lock graph is intentionally broader than a single executable.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import tomllib
from collections import defaultdict, deque
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "docs" / "licenses"
NOTICE_NAME = re.compile(r"^(?:licen[cs]e|copying|copyright|notice|unlicense)(?:[._-].*)?$", re.I)
NOTICE_DIR = {"license", "licenses", "licence", "licences", "copyright", "notices"}
SKIP_DIR = {".git", "target", "node_modules", ".cargo"}


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def run_json(args: list[str]) -> dict:
    return json.loads(subprocess.check_output(args, cwd=ROOT))


def metadata(path: str | None, platform: str | None = None) -> dict:
    if path:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    args = ["cargo", "metadata", "--locked", "--format-version", "1"]
    if platform:
        args += ["--filter-platform", platform]
    return run_json(args)


def notice_files(directory: Path, explicit: str | None = None) -> tuple[list[Path], list[str]]:
    files: set[Path] = set()
    warnings: list[str] = []
    if explicit:
        path = Path(explicit)
        path = path if path.is_absolute() else directory / path
        if path.is_file() and path.resolve().is_relative_to(directory.resolve()):
            files.add(path)
        else:
            warnings.append("Declared license_file is absent from the package archive.")
    for parent, dirs, names in os.walk(directory):
        dirs[:] = sorted(d for d in dirs if d not in SKIP_DIR)
        relative = Path(parent).relative_to(directory)
        in_license_directory = any(p.lower() in NOTICE_DIR for p in relative.parts)
        for name in sorted(names):
            if NOTICE_NAME.match(name) or in_license_directory:
                path = Path(parent) / name
                if path.is_file() and not path.is_symlink():
                    files.add(path)
    return sorted(files, key=lambda p: p.relative_to(directory).as_posix()), warnings


def append_files(parts: list[str], paths: list[Path], directory: Path) -> list[dict]:
    result = []
    for path in paths:
        data = path.read_bytes()
        item = {"path": path.relative_to(directory).as_posix(), "sha256": digest(data), "bytes": len(data)}
        try:
            content = data.decode("utf-8")
        except UnicodeDecodeError:
            # Preserve exact data without silently replacing copyright characters.
            import base64
            item["encoding"] = "base64 (upstream file was not UTF-8)"
            content = "Exact upstream file bytes, base64 encoded:\n" + base64.b64encode(data).decode("ascii")
        parts += [f"\n--- upstream file: {item['path']} ---\n", content, "\n--- end upstream file ---\n"]
        result.append(item)
    return result


def windows_roles(graph: dict) -> dict[str, list[str]]:
    """Conservative graph roles; includes all workspace binaries and their tests."""
    packages = {p["id"]: p for p in graph["packages"]}
    nodes = {n["id"]: n for n in graph["resolve"]["nodes"]}
    roots = set(graph["workspace_members"])
    roles: dict[str, set[str]] = defaultdict(set)
    pending = deque((root, "runtime") for root in roots)
    while pending:
        package_id, role = pending.popleft()
        if role in roles[package_id]:
            continue
        roles[package_id].add(role)
        for edge in nodes.get(package_id, {}).get("deps", []):
            target = packages[edge["pkg"]]
            proc_macro = any("proc-macro" in t["kind"] for t in target["targets"])
            for kind in edge["dep_kinds"]:
                dep_kind = kind["kind"] or "normal"
                if dep_kind == "dev" and package_id not in roots:
                    continue
                next_role = "test" if dep_kind == "dev" else "build" if dep_kind == "build" or proc_macro else role
                if role == "test" and dep_kind != "build":
                    next_role = "test"
                pending.append((edge["pkg"], next_role))
    return {key: sorted(value) for key, value in roles.items()}


def rust_notices(all_metadata: dict, windows_metadata: dict, sysroot: Path) -> dict:
    locked = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    lock_packages = {(p["name"], p["version"], p["source"]) for p in locked["package"] if p.get("source")}
    metadata_packages = {(p["name"], p["version"], p["source"]) for p in all_metadata["packages"] if p.get("source")}
    if lock_packages != metadata_packages:
        raise RuntimeError("Cargo metadata does not match the complete current frozen lockfile. Capture complete cargo metadata --locked again.")
    roles = windows_roles(windows_metadata)
    parts = [
        "rs-rize Rust third-party notices\n",
        "Generated from Cargo.lock and cargo metadata --locked.\n",
        "This collection includes normal, build, test, optional and other-platform locked crates.\n",
        "It is broader than the dependency subset linked into any individual executable.\n",
        "Per-package Windows roles are conservative graph categories; proc macros count as build dependencies.\n",
        "SPDX expressions are upstream metadata, not a new license grant for rs-rize.\n",
        "Full upstream files follow. Missing files are explicitly reported in rust-dependencies.json.\n",
    ]
    packages = []
    workspace = []
    for package in sorted(all_metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if package["source"] is None:
            workspace.append({"name": package["name"], "version": package["version"], "declaredLicense": package["license"]})
            continue
        directory = Path(package["manifest_path"]).parent
        files, warnings = notice_files(directory, package.get("license_file"))
        usage = roles.get(package["id"], ["other-target-or-optional"])
        parts += [f"\n{'=' * 78}\n{package['name']} {package['version']}\n",
                  f"Declared license: {package['license'] or '(not supplied)'}\n",
                  f"Source: {package['source']}\nWindows graph roles: {', '.join(usage)}\n"]
        if not files:
            parts += ["The published archive contains no standalone LICENSE/COPYING/COPYRIGHT/NOTICE file located by this generator. No copyright text has been invented.\n"]
            warnings.append("No standalone upstream license/notice file located.")
        notices = append_files(parts, files, directory)
        packages.append({"name": package["name"], "version": package["version"], "declaredLicense": package["license"],
                         "source": package["source"], "repository": package.get("repository"), "windowsGraphRoles": usage,
                         "upstreamNoticeFiles": notices, "warnings": warnings})

    # The Rust library/runtime copyright report includes third-party code used by
    # libstd across supported platforms. The compiler's much larger report is
    # deliberately not represented as code shipped with this application.
    rust_docs = sysroot / "share" / "doc" / "rust"
    toolchain_files = [rust_docs / "COPYRIGHT-library.html"]
    toolchain_files += sorted((rust_docs / "licenses").glob("*"))
    toolchain_files = [p for p in toolchain_files if p.is_file()]
    parts += [f"\n{'=' * 78}\nRust standard library / runtime notices\n",
              "The supplied library copyright report covers multiple platforms and is broader than a minimal binary.\n",
              "Its referenced upstream license texts are reproduced with it. HTML is retained verbatim as text.\n"]
    toolchain_notices = append_files(parts, toolchain_files, rust_docs) if toolchain_files else []
    if not toolchain_files:
        parts.append("No toolchain copyright documents were installed at the specified sysroot; obtain them from the Rust distribution used for the release.\n")
    rust_version = subprocess.check_output(["rustc", "--version"], text=True).strip()
    inventory = {"formatVersion": 1, "generatedBy": "scripts/license-notices.py", "cargoLockSha256": digest((ROOT / "Cargo.lock").read_bytes()),
                 "scope": "All registry/git packages in the frozen Cargo lock graph across platforms; Windows roles from filtered Cargo metadata include all workspace binaries, proc macros, build dependencies and tests. The list is broader than each shipped executable.",
                 "windowsTarget": "x86_64-pc-windows-msvc", "workspacePackages": workspace, "packages": packages,
                 "toolchain": {"version": rust_version, "scope": "Rust standard library/runtime copyright report across platforms; compiler and Cargo tool implementations are not bundled in the app.", "upstreamNoticeFiles": toolchain_notices,
                               "warnings": [] if toolchain_notices else ["Rust runtime copyright documents unavailable at the specified sysroot."]}}
    (OUT / "RUST-NOTICES.txt").write_text("".join(parts), encoding="utf-8", newline="\n")
    (OUT / "rust-dependencies.json").write_text(json.dumps(inventory, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")
    return inventory


def locate_npm(name: str, parent: Path) -> Path | None:
    for current in [parent, *parent.parents]:
        if not current.is_relative_to(ROOT):
            break
        path = current / "node_modules" / name
        if (path / "package.json").is_file():
            return path
    return None


def npm_notices() -> dict:
    declared = json.loads((ROOT / "package.json").read_text())
    lock = json.loads((ROOT / "package-lock.json").read_text())
    direct = set(declared.get("dependencies", {})) | set(declared.get("optionalDependencies", {}))
    pending = deque((name, ROOT) for name in sorted(direct))
    visited = set()
    packages = []
    parts = ["rs-rize npm production dependency notices\n",
             "Generated from actual installed package.json dependency/optional/peer graphs and package-lock.json.\n",
             "Build-only npm dependencies are excluded. Node is used to build/test the renderer, not as the application backend.\n"]
    while pending:
        name, parent = pending.popleft()
        directory = locate_npm(name, parent)
        if directory is None:
            # Optional peers can legitimately be absent. Direct packages cannot.
            if parent == ROOT:
                raise RuntimeError(f"Production dependency is not installed: {name}")
            continue
        key = directory.relative_to(ROOT).as_posix()
        if key in visited:
            continue
        visited.add(key)
        package = json.loads((directory / "package.json").read_text(encoding="utf-8"))
        package_license = package.get("license", package.get("licenses"))
        files, warnings = notice_files(directory)
        parts += [f"\n{'=' * 78}\n{package['name']} {package['version']}\nDeclared license: {json.dumps(package_license, ensure_ascii=False)}\n"]
        if not files:
            warnings.append("No standalone upstream license/notice file located.")
            parts += ["No standalone upstream license/notice file supplied; no attribution has been invented.\n"]
        notices = append_files(parts, files, directory)
        locked = lock.get("packages", {}).get(key, {})
        if locked.get("version") != package["version"]:
            raise RuntimeError(f"Installed {key} does not match package-lock.json")
        packages.append({"name": package["name"], "version": package["version"], "declaredLicense": package_license,
                         "directDependency": name in direct and key == "node_modules/" + name,
                         "installedPath": key, "source": locked.get("resolved"), "integrity": locked.get("integrity"),
                         "upstreamNoticeFiles": notices, "noticeFile": "docs/licenses/NPM-NOTICES.txt", "warnings": warnings})
        dependencies = set(package.get("dependencies", {})) | set(package.get("optionalDependencies", {})) | set(package.get("peerDependencies", {}))
        pending.extend((dependency, directory) for dependency in sorted(dependencies))
    packages.sort(key=lambda p: (p["name"], p["version"], p["installedPath"]))
    inventory = {"formatVersion": 1, "generatedBy": "scripts/license-notices.py", "packageLockSha256": digest((ROOT / "package-lock.json").read_bytes()),
                 "generatedFrom": "Actual installed production dependency, optional dependency and peer dependency graph rooted at package.json dependencies; build-only npm packages are excluded.", "packages": packages}
    (OUT / "NPM-NOTICES.txt").write_text("".join(parts), encoding="utf-8", newline="\n")
    (OUT / "production-dependencies.json").write_text(json.dumps(inventory, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")
    return inventory


def third_party_document(rust: dict, npm: dict) -> None:
    document = ROOT / "THIRD-PARTY-NOTICES.md"
    original = document.read_text(encoding="utf-8") if document.exists() else ""
    legacy_start, legacy_end = "<!-- legacy-v1.2-notices:start -->", "<!-- legacy-v1.2-notices:end -->"
    if legacy_start in original and legacy_end in original:
        legacy = original.split(legacy_start, 1)[1].split(legacy_end, 1)[0].strip()
    else:
        legacy = original.removeprefix("# Third-party notices").strip()
    no_files = [p for p in rust["packages"] if not p["upstreamNoticeFiles"]]
    header = f"""# Third-party notices

rs-rize is the Rust port of the published Rize Local v1.2 source. It retains the
React renderer and replaces the Electron/Node application and service logic.
The Rust and npm inventories below describe this repository's current dependency
graphs. They do not grant a license to rs-rize's own source; no project-wide
license has been selected by its owner.

The Rize name and unchanged public favicon remain associated with their owner.
The favicon's exact source and digest are retained in [asset provenance](docs/native-asset.txt).
No proprietary Rize application bundle is included.

## Current Rust dependencies

[RUST-NOTICES.txt](docs/licenses/RUST-NOTICES.txt) records **{len(rust['packages'])} locked external crates**
and reproduces their supplied upstream license, copyright, copying and notice
files, including normal, build, test and other-platform dependencies. The
[machine-readable inventory](docs/licenses/rust-dependencies.json) records exact
names, versions, SPDX declarations, package sources, notice digests and Windows
graph roles. This deliberately exceeds the subset linked into any one executable.
The Rust standard library's supplied copyright report and referenced license texts
are included; the compiler and Cargo programs are not bundled in the app.

{len(no_files)} crate archives supplied no standalone notice file located by the generator;
their exact declared license metadata and missing-file status are recorded without
inventing copyright text. The preserved upstream SPDX expressions can describe
alternative licenses; this document does not reinterpret those expressions.

## Current renderer dependencies

[NPM-NOTICES.txt](docs/licenses/NPM-NOTICES.txt) contains the actual installed
production package license files. The [production inventory](docs/licenses/production-dependencies.json)
contains exact installed versions and lockfile integrity values. npm packages used
only for building/testing are excluded from this renderer inventory; Node and
Electron are not application backend dependencies in this port.

| Package | Version | Upstream declared license |
| --- | --- | --- |
"""
    for package in npm["packages"]:
        declared = package["declaredLicense"]
        if not isinstance(declared, str):
            declared = json.dumps(declared, ensure_ascii=False)
        header += f"| {package['name']} | {package['version']} | {declared.replace('|', '&#124;')} |\n"
    header += """
## Retained timezone data and platform components

`crates/rize-service/src/windows_zones.json` retains the Windows-to-IANA timezone
mapping distributed as `windowsZones.json` by **node-ical 0.27.3**. Its parsed data
is unchanged; JSON formatting differs. node-ical declares Apache-2.0 and its package
author metadata names Jens Maus. Its exact supplied [Apache license](docs/licenses/node-ical-LICENSE.txt)
is preserved. node-ical's JavaScript runtime is not used by the Rust service.

The Windows desktop uses the operating system's Microsoft WebView2 runtime.
That runtime and any Microsoft bootstrapper retain Microsoft's own terms and
notices. They are platform components outside the Cargo/npm source inventories.
Installer tool outputs must retain their supplied notices as well.

Regenerate these inventories after lockfile changes with
`python scripts/license-notices.py` (Python 3.11+), using the release's Rust toolchain and an
installed npm graph (`npm ci`). The script reads actual downloaded crate archives
and installed package files; it records absent upstream texts instead of creating
attribution. It does not select an overall license for this project.

## Archived v1.2 notices

The following text and the other existing files in `docs/licenses` are retained
from the original Electron release for provenance. **This archival section
describes that earlier release, not the dependency graph or runtime shipped by
rs-rize.** The current inventories above supersede its package list.

"""
    document.write_text(header + legacy_start + "\n" + legacy + "\n" + legacy_end + "\n", encoding="utf-8", newline="\n")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metadata", help="Previously captured complete cargo metadata JSON")
    parser.add_argument("--windows-metadata", help="Previously captured Windows-filtered cargo metadata JSON")
    parser.add_argument("--sysroot", type=Path, help="Rust toolchain installation containing share/doc/rust")
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    all_metadata = metadata(args.metadata)
    windows_metadata = metadata(args.windows_metadata, "x86_64-pc-windows-msvc")
    sysroot = args.sysroot or Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
    rust = rust_notices(all_metadata, windows_metadata, sysroot)
    npm = npm_notices()
    third_party_document(rust, npm)
    print(json.dumps({"rustPackages": len(rust["packages"]), "rustMissingStandaloneNotices": sum(not p["upstreamNoticeFiles"] for p in rust["packages"]),
                      "npmProductionPackages": len(npm["packages"]), "rustNoticeBytes": (OUT / "RUST-NOTICES.txt").stat().st_size,
                      "npmNoticeBytes": (OUT / "NPM-NOTICES.txt").stat().st_size}))


if __name__ == "__main__":
    main()
