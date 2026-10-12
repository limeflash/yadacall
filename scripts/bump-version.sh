#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  printf 'Usage: %s <x.y.z>\n' "$0" >&2
  exit 2
fi

new_version="$1"
if [[ ! "$new_version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
  printf 'Version must be a stable x.y.z version (got %s)\n' "$new_version" >&2
  exit 2
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
python3 - "$repo_root" "$new_version" <<'PY'
from pathlib import Path
import re
import sys

root = Path(sys.argv[1])
version = sys.argv[2]

def replace_once(path: Path, pattern: str, replacement: str) -> str:
    contents = path.read_text(encoding="utf-8")
    updated, count = re.subn(pattern, replacement, contents, count=1, flags=re.MULTILINE)
    if count != 1:
        raise SystemExit(f"Expected one version field in {path.relative_to(root)}; found {count}")
    return updated

cargo_path = root / "Cargo.toml"
cargo_contents = cargo_path.read_text(encoding="utf-8")
workspace_pattern = r"(?ms)(^\[workspace\.package\]\s*\n)(.*?)(?=^\[|\Z)"
workspace_match = re.search(workspace_pattern, cargo_contents)
if workspace_match is None:
    raise SystemExit("Could not find [workspace.package] in Cargo.toml")
workspace_body, count = re.subn(
    r'(?m)^(version\s*=\s*")[^"]+("\s*)$',
    rf'\g<1>{version}\g<2>',
    workspace_match.group(2),
    count=1,
)
if count != 1:
    raise SystemExit("Expected one version field in [workspace.package]")
updated_cargo = (
    cargo_contents[:workspace_match.start(2)]
    + workspace_body
    + cargo_contents[workspace_match.end(2):]
)

files = [
    root / "apps/desktop/package.json",
    root / "apps/desktop/src-tauri/tauri.conf.json",
    root / "apps/desktop/ui/package.json",
]
json_pattern = r'^(\s*"version"\s*:\s*")[^"]+("\s*,?\s*)$'
updates = [(cargo_path, updated_cargo)]
for path in files:
    updates.append((path, replace_once(path, json_pattern, rf'\g<1>{version}\g<2>')))

for path, contents in updates:
    path.write_text(contents, encoding="utf-8")
print(f"Updated workspace and desktop package versions to {version}")
PY

# Keep local workspace package versions in Cargo.lock aligned with the manifest.
cargo update --workspace --manifest-path "$repo_root/Cargo.toml"
