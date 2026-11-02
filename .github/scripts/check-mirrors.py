#!/usr/bin/env python3
"""Check that the mirror releases this commit pins are published.

    check-mirrors.py            both mirrors, as the release workflow does
    check-mirrors.py --model    the model mirror alone

Devscriptor installs language servers from the LSP mirror release that
crates/lsp/src/manager/mirror.json pins, and falls back to the model mirror
release MODEL_MIRROR_RELEASE names in model_fetch.rs. A release of Devscriptor
pointing at a mirror that is not published, or not complete, would install
every server from its original source, so the release workflow runs this
before it publishes. Each LSP archive is asked for with HEAD and its size
compared with the one its manifest records; each model file the code pins is
downloaded and checked against the SHA-256 it pins.
"""

import hashlib
import json
import pathlib
import re
import sys
import urllib.error
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
RELEASES = "https://github.com/vladimirpesic/devscriptor/releases/download"
# PLATFORMS in crates/lsp/src/manager/mirror.rs.
PLATFORMS = ["linux-x86_64", "linux-aarch64", "macos-x86_64", "macos-aarch64", "windows-x86_64"]
# OPTIONAL_FILES in model_fetch.rs, which pins no SHA-256 for them.
OPTIONAL_MODEL_FILES = ["special_tokens_map.json"]


def size_at(url):
    """The size of the file at `url`, or why there is none."""
    request = urllib.request.Request(url, method="HEAD")
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return int(response.headers.get("Content-Length", -1))
    except urllib.error.HTTPError as error:
        return f"HTTP {error.code}"
    except OSError as error:
        return str(error)


def sha256_at(url):
    """The SHA-256 of the file at `url`, or why there is none."""
    digest = hashlib.sha256()
    try:
        with urllib.request.urlopen(url, timeout=600) as response:
            for chunk in iter(lambda: response.read(1 << 20), b""):
                digest.update(chunk)
    except urllib.error.HTTPError as error:
        return None, f"HTTP {error.code}"
    except OSError as error:
        return None, str(error)
    return digest.hexdigest(), None


def lsp_problems():
    """What keeps the LSP mirror release mirror.json pins from serving it."""
    problems = []
    pin = json.loads((ROOT / "crates/lsp/src/manager/mirror.json").read_text())
    release = pin.get("release")
    if not release:
        return ["mirror.json pins no LSP mirror release (scripts/pin-lsp-mirror.py)"]
    missing = [p for p in PLATFORMS if p not in pin.get("platforms", {})]
    if missing:
        problems.append(f"mirror.json pins no packages for {', '.join(missing)}")
    for platform, manifest in sorted(pin.get("platforms", {}).items()):
        for server, entry in sorted(manifest["servers"].items()):
            url = f"{RELEASES}/{release}/{entry['filename']}"
            size = size_at(url)
            expected = entry.get("size_bytes")
            if isinstance(size, str):
                problems.append(f"{platform} {server}: {url}: {size}")
            elif expected is not None and size != expected:
                problems.append(f"{platform} {server}: {url} is {size} bytes, not {expected}")
        print(f"{release}: checked {platform}'s {len(manifest['servers'])} archives")
    return problems


def model_problems():
    """What keeps the model mirror release model_fetch.rs names from serving
    the files it pins, as the code pins them."""
    source = (ROOT / "crates/core/src/embeddings/model_fetch.rs").read_text()
    release = re.search(r'^const MODEL_MIRROR_RELEASE: &str = "([^"]+)";$', source, re.M)
    if not release:
        return ["model_fetch.rs names no MODEL_MIRROR_RELEASE"]
    start = source.find("const REQUIRED_FILE_SHA256")
    table = source[start : source.find("];", start)] if start >= 0 else ""
    pins = re.findall(r'\(\s*"([^"]+)",\s*"([0-9a-f]{64})",?\s*\)', table)
    if not pins:
        return ["model_fetch.rs pins no file in REQUIRED_FILE_SHA256"]
    problems = []
    for path, pinned in pins:
        # The release holds the files side by side: `onnx/model_quantized.onnx`
        # is its `model_quantized.onnx`.
        url = f"{RELEASES}/{release[1]}/{path.rsplit('/', 1)[-1]}"
        actual, error = sha256_at(url)
        if error:
            problems.append(f"model: {url}: {error}")
        elif actual != pinned:
            problems.append(f"model: {url} has SHA-256 {actual}, not the pinned {pinned}")
    for name in OPTIONAL_MODEL_FILES:
        url = f"{RELEASES}/{release[1]}/{name}"
        size = size_at(url)
        if isinstance(size, str) or size <= 0:
            problems.append(f"model: {url}: {size}")
    print(f"{release[1]}: checked {len(pins)} pinned files and {len(OPTIONAL_MODEL_FILES)} other")
    return problems


def main():
    model_only = sys.argv[1:] == ["--model"]
    if sys.argv[1:] and not model_only:
        sys.exit(__doc__)
    problems = [] if model_only else lsp_problems()
    problems += model_problems()
    for problem in problems:
        print(f"::error::{problem}")
    sys.exit(1 if problems else 0)


if __name__ == "__main__":
    main()
