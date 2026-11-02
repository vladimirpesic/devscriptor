#!/usr/bin/env python3
"""Pin a release of the LSP mirror in crates/lsp/src/manager/mirror.json.

    scripts/pin-lsp-mirror.py <tag>
        Pins the release <tag> (lsp-servers-<date>-<commit>, as the LSP mirror workflow
        publishes it): every platform's manifest is read from the release.

    scripts/pin-lsp-mirror.py <tag> <manifest>...
        Pins the manifests given (lsp-packages/manifest-<platform>.json, as a
        packaging script writes it) under <tag>, and no other platform: to
        install packages that are not published yet, served at
        DEVSCRIPTOR_LSP_MIRROR_URL (docs/building.md).

--output <file> writes the pin somewhere other than mirror.json.
"""

import argparse
import json
import pathlib
import sys
import urllib.request

RELEASES = "https://github.com/vladimirpesic/devscriptor/releases/download"
# PLATFORMS in crates/lsp/src/manager/mirror.rs.
PLATFORMS = ["linux-x86_64", "linux-aarch64", "macos-x86_64", "macos-aarch64", "windows-x86_64"]
MIRROR_JSON = pathlib.Path(__file__).resolve().parent.parent / "crates/lsp/src/manager/mirror.json"


def checked(manifest, source):
    """The manifest, if it is one of a platform the mirror packages for."""
    platform = manifest.get("platform") if isinstance(manifest, dict) else None
    if platform not in PLATFORMS or not isinstance(manifest.get("servers"), dict):
        sys.exit(f"{source} is not a manifest of a platform the mirror packages for")
    for server_id, entry in manifest["servers"].items():
        if not isinstance(entry, dict) or not all(
            isinstance(entry.get(key), str) for key in ("filename", "sha256")
        ):
            sys.exit(f"{source}: {server_id} names no archive and checksum")
    return manifest


def fetch(tag, platform):
    url = f"{RELEASES}/{tag}/manifest-{platform}.json"
    try:
        with urllib.request.urlopen(url, timeout=60) as response:
            manifest = checked(json.load(response), url)
    except OSError as error:
        sys.exit(f"{url}: {error}")
    if manifest["platform"] != platform:
        sys.exit(f"{url} is the manifest of {manifest['platform']}")
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("tag")
    parser.add_argument("manifests", nargs="*", type=pathlib.Path)
    parser.add_argument("--output", type=pathlib.Path, default=MIRROR_JSON)
    args = parser.parse_args()
    if not args.tag.startswith("lsp-servers-") or "/" in args.tag:
        sys.exit(f"{args.tag} is not a tag of the LSP mirror (lsp-servers-<date>-<commit>)")

    if args.manifests:
        manifests = [
            checked(json.loads(path.read_text(encoding="utf-8-sig")), path)
            for path in args.manifests
        ]
    else:
        manifests = [fetch(args.tag, platform) for platform in PLATFORMS]

    platforms = {}
    for manifest in sorted(manifests, key=lambda m: PLATFORMS.index(m["platform"])):
        if manifest["platform"] in platforms:
            sys.exit(f"two manifests of {manifest['platform']}")
        manifest["servers"] = dict(sorted(manifest["servers"].items()))
        platforms[manifest["platform"]] = manifest
    pin = {"release": args.tag, "platforms": platforms}
    args.output.write_text(json.dumps(pin, indent=2) + "\n", encoding="utf-8")
    print(f"Pinned {args.tag}: {', '.join(platforms)} in {args.output}")


if __name__ == "__main__":
    main()
