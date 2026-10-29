#!/usr/bin/env python3
"""Check that the mirror releases this commit pins are published.

Devscriptor installs language servers from the LSP mirror release that
crates/lsp/src/manager/mirror.json pins, and falls back to the model mirror
release MODEL_MIRROR_RELEASE names in model_fetch.rs. A release of Devscriptor
pointing at a mirror that is not published, or not complete, would install
every server from its original source, so the release workflow runs this
before it publishes. Each file is asked for with HEAD, and an LSP archive's size
is compared with the one its manifest records.
"""

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
# REQUIRED_FILES and OPTIONAL_FILES in model_fetch.rs, side by side.
MODEL_FILES = [
    "model_quantized.onnx",
    "tokenizer.json",
    "config.json",
    "tokenizer_config.json",
    "special_tokens_map.json",
]


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


def main():
    problems = []

    pin = json.loads((ROOT / "crates/lsp/src/manager/mirror.json").read_text())
    release = pin.get("release")
    if not release:
        problems.append("mirror.json pins no LSP mirror release (scripts/pin-lsp-mirror.py)")
    else:
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

    source = (ROOT / "crates/core/src/embeddings/model_fetch.rs").read_text()
    model = re.search(r'^const MODEL_MIRROR_RELEASE: &str = "([^"]+)";$', source, re.M)
    if not model:
        problems.append("model_fetch.rs names no MODEL_MIRROR_RELEASE")
    else:
        for name in MODEL_FILES:
            url = f"{RELEASES}/{model[1]}/{name}"
            size = size_at(url)
            if isinstance(size, str) or size <= 0:
                problems.append(f"model: {url}: {size}")
        print(f"{model[1]}: checked {len(MODEL_FILES)} files")

    for problem in problems:
        print(f"::error::{problem}")
    sys.exit(1 if problems else 0)


if __name__ == "__main__":
    main()
