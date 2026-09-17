#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

current="$(awk -F '"' '
    $0 == "[package]" { in_package = 1; next }
    in_package && /^version = "/ { print $2; exit }
' Cargo.toml)"

if [[ ! "$current" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
    echo "Unsupported current version: $current" >&2
    exit 1
fi

major="${BASH_REMATCH[1]}"
minor="${BASH_REMATCH[2]}"
patch="${BASH_REMATCH[3]}"
requested="${1:-patch}"

case "$requested" in
    patch) next_version="$major.$minor.$((10#$patch + 1))" ;;
    minor) next_version="$major.$((10#$minor + 1)).0" ;;
    major) next_version="$((10#$major + 1)).0.0" ;;
    *)
        if [[ ! "$requested" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
            echo "Usage: just up-version [patch|minor|major|X.Y.Z]" >&2
            exit 1
        fi
        next_version="$requested"
        ;;
esac

if [[ "$next_version" == "$current" ]]; then
    echo "Version is already $current" >&2
    exit 1
fi

temp_file="$(mktemp "${TMPDIR:-/tmp}/vrchat-x3d-start-fix-version.XXXXXX")"
trap 'rm -f "$temp_file"' EXIT

awk -v next_version="$next_version" '
    $0 == "[package]" { in_package = 1 }
    in_package && !changed && /^version = "[^"]+"$/ {
        print "version = \"" next_version "\""
        changed = 1
        next
    }
    { print }
    END { if (!changed) exit 1 }
' Cargo.toml > "$temp_file"

mv "$temp_file" Cargo.toml
cargo metadata --no-deps --format-version 1 >/dev/null

echo "Version bumped: $current -> $next_version"

