#!/usr/bin/env python3
"""Validate the checked-in p-track Help Center using only Python stdlib."""

from __future__ import annotations

import argparse
import hashlib
from html.parser import HTMLParser
import json
from pathlib import Path
import posixpath
import re
import struct
import sys
import tomllib
from urllib.parse import unquote, urlsplit


REPO = Path(__file__).resolve().parent.parent
HELP = REPO / "docs" / "help"
SCREENSHOTS = HELP / "assets" / "screenshots"
EXTERNAL_PATH_PREFIXES = {
    "github.com": "/ro-ag/ptrack",
    "ro-ag.github.io": "/ptrack/help/",
}


class HelpHTMLParser(HTMLParser):
    def __init__(self, path: Path) -> None:
        super().__init__(convert_charrefs=True)
        self.path = path
        self.ids: list[str] = []
        self.links: list[tuple[str, str]] = []
        self.images: list[dict[str, str]] = []
        self.headings: list[int] = []
        self.h1_count = 0
        self.main_count = 0
        self.lang = ""
        self.title_depth = 0
        self.title_text: list[str] = []
        self.viewport = False
        self.skip_main = False
        self.labeled_nav = False
        self.labeled_search = False
        self.anchor_stack: list[list[str]] = []
        self.anchor_texts: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        values = {key: value or "" for key, value in attrs}
        if tag == "html":
            self.lang = values.get("lang", "")
        if tag == "title":
            self.title_depth += 1
        if tag == "meta" and values.get("name", "").lower() == "viewport":
            self.viewport = bool(values.get("content", "").strip())
        if tag == "main":
            self.main_count += 1
        if tag == "nav" and values.get("aria-label", "").strip():
            self.labeled_nav = True
        if tag == "form" and (
            values.get("role") == "search" or values.get("aria-label", "").strip()
        ):
            self.labeled_search = True
        if tag in {"h1", "h2", "h3", "h4", "h5", "h6"}:
            level = int(tag[1])
            self.headings.append(level)
            if level == 1:
                self.h1_count += 1
        element_id = values.get("id", "").strip()
        if element_id:
            self.ids.append(element_id)
        if tag == "a":
            href = values.get("href", "").strip()
            if href:
                self.links.append(("href", href))
            if href == "#main":
                self.skip_main = True
            if values.get("target") == "_blank":
                rel = set(values.get("rel", "").lower().split())
                if not {"noopener", "noreferrer"}.issubset(rel):
                    self.links.append(("unsafe-target", href))
            self.anchor_stack.append([])
        if tag == "link":
            href = values.get("href", "").strip()
            if href:
                self.links.append(("href", href))
        for attr in ("src",):
            target = values.get(attr, "").strip()
            if target:
                self.links.append((attr, target))
        if tag == "img":
            self.images.append(values)

    def handle_endtag(self, tag: str) -> None:
        if tag == "title" and self.title_depth:
            self.title_depth -= 1
        if tag == "a" and self.anchor_stack:
            text = " ".join("".join(self.anchor_stack.pop()).split())
            self.anchor_texts.append(text)

    def handle_data(self, data: str) -> None:
        if self.title_depth:
            self.title_text.append(data)
        if self.anchor_stack:
            self.anchor_stack[-1].append(data)


class Validation:
    def __init__(self) -> None:
        self.errors: list[str] = []

    def require(self, condition: bool, message: str) -> None:
        if not condition:
            self.errors.append(message)


def json_file(path: Path) -> dict:
    with path.open(encoding="utf-8") as source:
        return json.load(source)


def allowed_external_url(raw_target: str) -> bool:
    """Accept only canonical HTTPS URLs inside p-track's published namespaces."""
    parts = urlsplit(raw_target)
    try:
        port = parts.port
    except ValueError:
        return False
    if (
        parts.scheme != "https"
        or parts.hostname not in EXTERNAL_PATH_PREFIXES
        or parts.username is not None
        or parts.password is not None
        or port is not None
        or parts.query
    ):
        return False
    path = unquote(parts.path)
    if path != parts.path or "\\" in path:
        return False
    canonical = posixpath.normpath(path)
    if path not in {canonical, f"{canonical}/"}:
        return False
    prefix = EXTERNAL_PATH_PREFIXES[parts.hostname]
    if parts.hostname == "github.com":
        return canonical == prefix or canonical.startswith(f"{prefix}/")
    return path.startswith(prefix)


def html_documents(validation: Validation) -> dict[Path, HelpHTMLParser]:
    documents: dict[Path, HelpHTMLParser] = {}
    for path in sorted(HELP.rglob("*.html")):
        parser = HelpHTMLParser(path)
        try:
            parser.feed(path.read_text(encoding="utf-8"))
            parser.close()
        except Exception as error:  # pragma: no cover - defensive parser report
            validation.errors.append(f"{path.relative_to(REPO)}: HTML parse failed: {error}")
        documents[path.resolve()] = parser
    validation.require(bool(documents), "docs/help: no HTML pages found")
    return documents


def route_target(source: Path, raw_target: str) -> tuple[Path, str] | None:
    parts = urlsplit(raw_target)
    if parts.scheme or parts.netloc:
        return None
    path_text = unquote(parts.path)
    if path_text.startswith("/"):
        raise ValueError("root-absolute links break the /ptrack project-site prefix")
    candidate = (source.parent / path_text).resolve() if path_text else source.resolve()
    try:
        candidate.relative_to(HELP.resolve())
    except ValueError as error:
        raise ValueError("link escapes docs/help") from error
    if candidate.is_dir():
        candidate = candidate / "index.html"
    elif not candidate.exists() and not candidate.suffix:
        candidate = candidate / "index.html"
    return candidate, unquote(parts.fragment)


def check_links(validation: Validation, documents: dict[Path, HelpHTMLParser]) -> None:
    for source, parser in documents.items():
        label = source.relative_to(REPO)
        for kind, target in parser.links:
            if kind == "unsafe-target":
                validation.errors.append(
                    f"{label}: target=_blank requires rel=noopener noreferrer: {target}"
                )
                continue
            parts = urlsplit(target)
            if parts.scheme or parts.netloc:
                validation.require(
                    allowed_external_url(target),
                    f"{label}: external link must use an approved p-track HTTPS path: {target}",
                )
                continue
            try:
                resolved = route_target(source, target)
            except ValueError as error:
                validation.errors.append(f"{label}: {target}: {error}")
                continue
            if resolved is None:
                continue
            destination, fragment = resolved
            validation.require(
                destination.exists(),
                f"{label}: missing local target {target} ({destination.relative_to(REPO)})",
            )
            if fragment and destination.exists():
                target_parser = documents.get(destination.resolve())
                validation.require(
                    target_parser is not None and fragment in target_parser.ids,
                    f"{label}: missing fragment #{fragment} in {destination.relative_to(REPO)}",
                )


def check_accessibility(validation: Validation, documents: dict[Path, HelpHTMLParser]) -> None:
    weak_anchor = re.compile(r"^(click|here|more|link|read more)$", re.IGNORECASE)
    for path, parser in documents.items():
        label = path.relative_to(REPO)
        validation.require(parser.lang == "en", f"{label}: html lang must be en")
        validation.require(bool("".join(parser.title_text).strip()), f"{label}: missing title")
        validation.require(parser.viewport, f"{label}: missing viewport meta")
        validation.require(parser.main_count == 1, f"{label}: expected exactly one main")
        validation.require(parser.h1_count == 1, f"{label}: expected exactly one h1")
        validation.require(parser.skip_main, f"{label}: missing skip link to #main")
        validation.require(parser.labeled_nav, f"{label}: navigation needs an aria-label")
        validation.require(parser.labeled_search, f"{label}: search form needs a label or role")
        validation.require(len(parser.ids) == len(set(parser.ids)), f"{label}: duplicate IDs")
        previous = 0
        for level in parser.headings:
            if previous and level > previous + 1:
                validation.errors.append(
                    f"{label}: heading level jumps from h{previous} to h{level}"
                )
            previous = level
        for image in parser.images:
            validation.require(
                bool(image.get("alt", "").strip()),
                f"{label}: image {image.get('src', '<unknown>')} needs nonempty alt text",
            )
            if image.get("src", "").endswith(".png"):
                validation.require(
                    image.get("width", "").isdigit() and image.get("height", "").isdigit(),
                    f"{label}: PNG image needs intrinsic width and height",
                )
        for text in parser.anchor_texts:
            validation.require(
                bool(text) and not weak_anchor.fullmatch(text),
                f"{label}: anchor text is empty or unhelpful: {text!r}",
            )


def stable_version(validation: Validation) -> str:
    changelog = (REPO / "CHANGELOG.md").read_text(encoding="utf-8")
    match = re.search(r"^## \[(\d+\.\d+\.\d+)\]", changelog, re.MULTILINE)
    validation.require(match is not None, "CHANGELOG.md: stable release heading not found")
    return match.group(1) if match else ""


def check_version(validation: Validation, documents: dict[Path, HelpHTMLParser]) -> str:
    version = stable_version(validation)
    readme = (REPO / "README.md").read_text(encoding="utf-8")
    tauri = json_file(REPO / "src-tauri" / "tauri.conf.json")
    with (REPO / "src-tauri" / "Cargo.toml").open("rb") as source:
        desktop_manifest = tomllib.load(source)
    with (REPO / "crates" / "ptrack-cli" / "Cargo.toml").open("rb") as source:
        cli_manifest = tomllib.load(source)
    site = json_file(HELP / "site.json")
    search = json_file(HELP / "search-index.json")
    manifest = json_file(SCREENSHOTS / "manifest.json")
    validation.require(f"release-v{version}-" in readme, "README.md: release badge is stale")
    validation.require(f"help-v{version}-" in readme, "README.md: Help badge is stale")
    validation.require(tauri.get("version") == version, "src-tauri/tauri.conf.json: version is stale")
    validation.require(desktop_manifest.get("package", {}).get("version") == version, "src-tauri/Cargo.toml: version is stale")
    validation.require(cli_manifest.get("package", {}).get("version") == version, "ptrack-cli/Cargo.toml: version is stale")
    validation.require(site.get("productVersion") == version, "docs/help/site.json: productVersion is stale")
    validation.require(search.get("productVersion") == version, "docs/help/search-index.json: productVersion is stale")
    validation.require(manifest.get("productVersion") == version, "screenshot manifest: productVersion is stale")
    for path, parser in documents.items():
        source = path.read_text(encoding="utf-8")
        validation.require(
            f'<meta name="ptrack-version" content="{version}">' in source,
            f"{path.relative_to(REPO)}: ptrack-version meta is stale",
        )
        validation.require(
            f"v{version}" in source,
            f"{path.relative_to(REPO)}: visible version badge is stale",
        )
    return version


def png_dimensions(path: Path) -> tuple[int, int]:
    with path.open("rb") as source:
        header = source.read(24)
    if len(header) != 24 or header[:8] != b"\x89PNG\r\n\x1a\n" or header[12:16] != b"IHDR":
        raise ValueError("not a PNG with an IHDR header")
    return struct.unpack(">II", header[16:24])


def source_digest(paths: list[str]) -> str:
    digest = hashlib.sha256()
    for relative in paths:
        path = REPO / relative
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
    return digest.hexdigest()


def check_screenshots(
    validation: Validation,
    documents: dict[Path, HelpHTMLParser],
    version: str,
) -> None:
    manifest = json_file(SCREENSHOTS / "manifest.json")
    entries = manifest.get("screenshots", [])
    names = [entry.get("file", "") for entry in entries]
    validation.require(len(names) == len(set(names)), "screenshot manifest: duplicate files")
    referenced: set[str] = set()
    for source, parser in documents.items():
        for image in parser.images:
            src = image.get("src", "")
            if "assets/screenshots/" not in src:
                continue
            try:
                target = route_target(source, src)
            except ValueError:
                continue
            if target:
                referenced.add(target[0].name)
    validation.require(referenced == set(names), "screenshot manifest and HTML image references differ")
    actual_pngs = {path.name for path in SCREENSHOTS.glob("*.png")}
    validation.require(actual_pngs == set(names), "screenshot manifest and PNG directory differ")

    themes: dict[str, set[str]] = {}
    for entry in entries:
        name = entry.get("file", "")
        path = SCREENSHOTS / name
        validation.require(path.is_file(), f"screenshot manifest: missing {name}")
        if not path.is_file():
            continue
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        validation.require(digest == entry.get("sha256"), f"screenshot manifest: stale hash for {name}")
        try:
            width, height = png_dimensions(path)
        except ValueError as error:
            validation.errors.append(f"screenshot manifest: {name}: {error}")
            continue
        validation.require(
            (width, height) == (entry.get("width"), entry.get("height")),
            f"screenshot manifest: stale dimensions for {name}",
        )
        workflow = entry.get("workflow", "")
        theme = entry.get("theme", "")
        themes.setdefault(workflow, set()).add(theme)
        validation.require(
            isinstance(entry.get("route"), str) and bool(entry["route"]),
            f"screenshot manifest: {name} needs a route",
        )
    for workflow in manifest.get("requiredWorkflows", []):
        validation.require(
            themes.get(workflow) == {"dark", "light"},
            f"screenshot manifest: {workflow} needs current dark and light captures",
        )
    validation.require(manifest.get("productVersion") == version, "screenshot manifest: version mismatch")
    sources = manifest.get("uiSources", [])
    validation.require(
        source_digest(sources) == manifest.get("uiSourceSha256"),
        "screenshot manifest: UI sources changed; recapture or review and refresh the digest",
    )


def check_routes(validation: Validation) -> None:
    site = json_file(HELP / "site.json")
    search = json_file(HELP / "search-index.json")
    navigation = site.get("navigation", [])
    routes = {entry.get("url") for entry in navigation}
    search_routes = {entry.get("url") for entry in search.get("entries", [])}
    validation.require(routes.issubset(search_routes), "search index is missing a navigation route")
    for route in routes | search_routes:
        if not isinstance(route, str):
            validation.errors.append(f"site route must be a string: {route}")
            continue
        try:
            resolved = route_target(HELP / "index.html", route)
        except ValueError as error:
            validation.errors.append(f"site route {route}: {error}")
            continue
        if resolved is None:
            validation.errors.append(f"site route must be local: {route}")
            continue
        destination, fragment = resolved
        validation.require(not fragment, f"site route must not include a fragment: {route}")
        validation.require(
            destination.is_file() and destination.name == "index.html",
            f"site route is missing a directory index: {route}",
        )
    destinations = site.get("nativeDestinations", {})
    native_source = (REPO / "crates" / "ptrack-app" / "src" / "desktop_runtime.rs").read_text(encoding="utf-8")
    for name, target in destinations.items():
        validation.require(target in native_source, f"native destination {name} differs from Rust allowlist")
        parsed = urlsplit(target)
        if parsed.hostname == "ro-ag.github.io":
            prefix = "/ptrack/help/"
            validation.require(parsed.path.startswith(prefix), f"native destination {name} escapes Help Center")
            local = HELP / parsed.path.removeprefix(prefix)
            if local.is_dir():
                local = local / "index.html"
            validation.require(local.exists(), f"native destination {name} has no local route")
            if parsed.fragment and local.exists():
                parser = HelpHTMLParser(local)
                parser.feed(local.read_text(encoding="utf-8"))
                validation.require(parsed.fragment in parser.ids, f"native destination {name} has a stale fragment")


def run_all() -> int:
    validation = Validation()
    documents = html_documents(validation)
    check_links(validation, documents)
    check_accessibility(validation, documents)
    version = check_version(validation, documents)
    check_screenshots(validation, documents, version)
    check_routes(validation)
    if validation.errors:
        for error in validation.errors:
            print(f"ERROR: {error}", file=sys.stderr)
        print(f"Help Center validation failed with {len(validation.errors)} error(s).", file=sys.stderr)
        return 1
    print(
        f"Help Center validation passed: {len(documents)} pages, "
        f"version {version}, links, accessibility contracts, routes, and screenshots."
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("check", nargs="?", choices=("all",), default="all")
    parser.parse_args()
    return run_all()


if __name__ == "__main__":
    raise SystemExit(main())
