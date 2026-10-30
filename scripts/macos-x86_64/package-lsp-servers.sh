#!/usr/bin/env bash

# Package LSP Servers Script for macOS x86_64 (Intel)
# Creates .tar.gz archives for each LSP server
# Usage: ./package-lsp-servers.sh

set -e
set -o pipefail

PLATFORM="macos-x86_64"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LSP_SERVERS_DIR="${SCRIPT_DIR}/lsp-servers"
OUTPUT_DIR="${SCRIPT_DIR}/lsp-packages"
# The registry: each server's license files, and the servers the manifest is
# checked against.
REGISTRY_JSON="${DEVSCRIPTOR_REGISTRY_JSON:-${SCRIPT_DIR}/../../crates/lsp/src/manager/registry.json}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

print_error() { echo -e "${RED}ERROR: $1${NC}" >&2; }
print_success() { echo -e "${GREEN}✓ $1${NC}"; }
print_info() { echo -e "${BLUE}ℹ $1${NC}"; }
print_warning() { echo -e "${YELLOW}⚠ $1${NC}"; }

# sha256_of <file>: the file's SHA-256.
sha256_of() { shasum -a 256 "$1" | cut -d' ' -f1; }

# The version of a packaged server: what its download recorded in `.version`
# (every pinned build, and clangd's release), else what rust-analyzer and gopls
# report, else the day it is packaged.
get_server_version() {
    local server_id="$1"
    local server_dir="${LSP_SERVERS_DIR}/${server_id}"
    local version="" binary

    if [[ -f "${server_dir}/.version" ]]; then
        version=$(tr -d '[:space:]' < "${server_dir}/.version")
    fi
    if [[ -z "$version" ]]; then
        case "$server_id" in
            "rust-analyzer")
                binary="${server_dir}/bin/rust-analyzer"
                # `rust-analyzer 0.3.2963-standalone (...)`
                [[ -x "$binary" ]] && version=$("$binary" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)
                ;;
            "gopls")
                binary="${server_dir}/bin/gopls"
                [[ -x "$binary" ]] && version=$("$binary" version 2>/dev/null | grep -oE 'v[0-9]+\.[0-9]+\.[0-9]+' | head -1 | sed 's/^v//' || true)
                ;;
        esac
    fi
    if [[ -z "$version" ]]; then
        version=$(date +%Y-%m-%d)
    fi
    echo "$version"
}

# The servers this run packaged, a `<id> <archive> <version>` line each: the
# checksums and the manifest are of these and only these.
PACKAGED=""

# add_license_files <id>: put the server's license files, as registry.json
# names them, in its LICENSES/: copied from the server's own files where it
# ships them, else fetched from a commit of its publisher's repository and
# checked against the SHA-256 the registry pins, each under its own name or
# the `name` the registry gives it. A server without them is not packaged.
add_license_files() {
    local server_id="$1"
    local server_dir="${LSP_SERVERS_DIR}/${server_id}"
    local licenses="${server_dir}/LICENSES"
    local path url sha256 name found dest count=0
    rm -rf "${licenses:?}"
    mkdir -p "$licenses"
    while IFS=$'\x1f' read -r path url sha256 name; do
        if [[ -n "$path" ]]; then
            # A glob where a directory is named by a version.
            found=$(compgen -G "${server_dir}/${path}" | head -1 || true)
            if [[ -z "$found" || ! -f "$found" ]]; then
                print_error "${server_id}: no ${path} to take its license from"
                return 1
            fi
            cp "$found" "${licenses}/${name:-$(basename "$found")}"
        else
            dest="${licenses}/${name:-${url##*/}}"
            if ! curl -fsSL --retry 3 "$url" -o "$dest" || [[ "$(sha256_of "$dest")" != "$sha256" ]]; then
                rm -f "$dest"
                print_error "${server_id}: ${url} could not be fetched, or is not the file the registry pins"
                return 1
            fi
        fi
        count=$((count + 1))
    done < <(jq -r --arg id "$server_id" \
        '(.servers // .)[$id].license_files // [] | .[] | [.path // "", .url // "", .sha256 // "", .name // ""] | join("\u001f")' \
        "$REGISTRY_JSON")
    if [[ $count -eq 0 ]]; then
        print_error "${server_id}: registry.json names no license file"
        return 1
    fi
}

# Package a single server
package_server() {
    local server_id="$1"
    local version archive_name archive_path size
    version=$(get_server_version "$server_id")
    archive_name="${server_id}-${version}-${PLATFORM}.tar.gz"
    archive_path="${OUTPUT_DIR}/${archive_name}"

    print_info "Packaging ${server_id} (version: ${version})..."
    add_license_files "$server_id" || return 1
    # macOS's tar adds a `._<name>` AppleDouble file per file with extended
    # attributes, and the attributes themselves as headers other tars warn
    # about; neither belongs in a package.
    if COPYFILE_DISABLE=1 tar --no-xattrs --no-mac-metadata -czf "${archive_path}" -C "${LSP_SERVERS_DIR}" "${server_id}"; then
        # The archive's own size: du counts the blocks allocated to it, which
        # on ZFS lag behind a file just written.
        size=$(ls -lh "$archive_path" | awk '{print $5}')
        PACKAGED+="${server_id} ${archive_name} ${version}"$'\n'
        print_success "Created: ${archive_name} (${size})"
    else
        rm -f "$archive_path"
        print_error "Failed to create archive for ${server_id}"
        return 1
    fi
}

# Generate checksums
generate_checksums() {
    print_info "Generating checksums..."
    local checksum_file="${OUTPUT_DIR}/sha256sums-${PLATFORM}.txt"
    local id archive version
    : > "$checksum_file"
    while read -r id archive version; do
        [[ -n "$archive" ]] || continue
        printf '%s  %s\n' "$(sha256_of "${OUTPUT_DIR}/${archive}")" "$archive" >> "$checksum_file"
    done <<< "$PACKAGED"
    print_success "Checksums written to: ${checksum_file}"
}

# Create the JSON manifest the mirror pins in crates/lsp/src/manager/mirror.json.
#
# Schema MUST match PlatformManifest in crates/lsp/src/manager/mirror.rs:
#   { "platform": ..., "generated": ..., "servers": {
#       "<registry-id>": { "filename": ..., "sha256": ...,
#                          "size_bytes": N, "version": ... } } }
#
# Each server's directory is named by its registry id, which keys it here.
create_manifest() {
    print_info "Creating manifest..."
    local manifest_file="${OUTPUT_DIR}/manifest-${PLATFORM}.json"
    local timestamp entries_file id archive version
    timestamp=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
    entries_file=$(mktemp)

    while read -r id archive version; do
        [[ -n "$archive" ]] || continue
        # One {id, entry} record per server; assembled into a map below.
        jq -n \
            --arg id "$id" \
            --arg filename "$archive" \
            --arg sha256 "$(sha256_of "${OUTPUT_DIR}/${archive}")" \
            --argjson size_bytes "$(stat -f%z "${OUTPUT_DIR}/${archive}")" \
            --arg version "$version" \
            '{id: $id, entry: {filename: $filename, sha256: $sha256, size_bytes: $size_bytes, version: $version}}' \
            >> "$entries_file"
    done <<< "$PACKAGED"

    # Fold the records into a map keyed by registry ID and wrap with metadata.
    jq -s \
        --arg platform "$PLATFORM" \
        --arg generated "$timestamp" \
        '{platform: $platform, generated: $generated, servers: (map({(.id): .entry}) | add // {})}' \
        "$entries_file" > "$manifest_file"

    rm -f "$entries_file"

    print_success "Manifest created: ${manifest_file}"
}

# Warn (don't fail) when the manifest IDs drift from the canonical registry.
validate_against_registry() {
    local manifest_file="${OUTPUT_DIR}/manifest-${PLATFORM}.json"
    local reg_ids man_ids unknown absent
    # A server that comes with its toolchain (sourcekit-lsp), or whose builds
    # expire (kotlin-lsp), is never packaged.
    reg_ids=$(jq -r '(.servers // .) | to_entries[] | select(.value.source.type != "system" and (.value.builds_expire | not)) | .key' "$REGISTRY_JSON" | sort)
    man_ids=$(jq -r '.servers | keys[]' "$manifest_file" | sort)
    unknown=$(comm -23 <(echo "$man_ids") <(echo "$reg_ids"))
    absent=$(comm -13 <(echo "$man_ids") <(echo "$reg_ids"))

    if [[ -n "$unknown" ]]; then
        print_warning "Manifest IDs NOT in registry (can never be requested — name its directory after its registry id):"
        echo "$unknown" | sed 's/^/    - /'
    fi
    if [[ -n "$absent" ]]; then
        print_warning "Registry servers absent from this manifest (no mirror copy → Tier-2 fallback):"
        echo "$absent" | sed 's/^/    - /'
    fi
    if [[ -z "$unknown" && -z "$absent" ]]; then
        print_success "Manifest IDs match registry exactly ($(echo "$reg_ids" | grep -c .) servers)"
    fi
}

# Main
main() {
    print_info "LSP Server Packaging Script for ${PLATFORM}"
    print_info "=========================================="

    if [[ ! -f "$REGISTRY_JSON" ]]; then
        print_error "registry.json not found at ${REGISTRY_JSON}; it names each server's license files"
        print_info "Set DEVSCRIPTOR_REGISTRY_JSON to point at it"
        exit 1
    fi
    if [[ ! -d "$LSP_SERVERS_DIR" ]]; then
        print_error "LSP servers directory not found: ${LSP_SERVERS_DIR}"
        print_info "Please run: ./download-lsp-servers.sh"
        exit 1
    fi

    mkdir -p "$OUTPUT_DIR"
    # An earlier run's archives would be checksummed and uploaded with this
    # run's, and its manifest would name servers this run no longer packages.
    rm -f "${OUTPUT_DIR}"/*-"${PLATFORM}".tar.gz \
        "${OUTPUT_DIR}/sha256sums-${PLATFORM}.txt" "${OUTPUT_DIR}/manifest-${PLATFORM}.json"

    local server_count=0 success_count=0 fail_count=0 server_id
    for server_dir in "${LSP_SERVERS_DIR}"/*/; do
        [[ -d "$server_dir" ]] || continue
        server_id=$(basename "$server_dir")
        # An install a download left unfinished (`<id>.new`) is no server.
        [[ "$server_id" == *.new ]] && continue
        server_count=$((server_count + 1))
        if package_server "$server_id"; then
            success_count=$((success_count + 1))
        else
            fail_count=$((fail_count + 1))
        fi
    done

    echo ""
    print_info "Packaging Summary"
    print_info "================="
    print_info "Total servers: ${server_count}"
    print_success "Successfully packaged: ${success_count}"
    if [[ $fail_count -gt 0 ]]; then
        print_warning "Failed: ${fail_count}"
    fi

    echo ""
    generate_checksums
    create_manifest
    validate_against_registry

    echo ""
    print_success "Packaging complete!"
    print_info "Archives location: ${OUTPUT_DIR}"
    print_info "To install from them, see \"Testing packages locally\" in docs/building.md"
}

main "$@"
