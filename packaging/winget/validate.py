#!/usr/bin/env python3
"""Split the combined winget manifest and validate it against the schemas.

`manifest.yaml` keeps winget's three documents (version, defaultLocale,
installer) in one file so it can be reviewed as a unit. Submission needs them
as three files under `manifests/b/bettershot/<version>/`, and that split is a
step nobody performs until release day.

Doing it here checks the thing that actually goes wrong: a key in the wrong
document, a misspelled field, an enum winget does not accept. `winget validate`
would be the natural tool, but it is not present on the Windows Server images,
and the schemas it validates against are published, so this uses those
directly.

What it cannot check is the installer URL and its SHA256, which do not exist
until a release does.

Run from the repository root.
"""

from __future__ import annotations

import json
import pathlib
import sys
import urllib.request

import jsonschema
import yaml

HERE = pathlib.Path(__file__).parent
MANIFEST = HERE / "manifest.yaml"

SCHEMA_BASE = (
    "https://raw.githubusercontent.com/microsoft/winget-cli/master/schemas/JSON/manifests"
)

# ManifestType -> (schema file, submission file suffix)
KINDS = {
    "version": ("manifest.version.{v}.json", "yaml"),
    "defaultLocale": ("manifest.defaultLocale.{v}.json", "locale.en-US.yaml"),
    "installer": ("manifest.installer.{v}.json", "installer.yaml"),
}

# Values that are placeholders until there is a release to point at. Their
# presence is checked; their content cannot be.
PLACEHOLDERS = {
    "InstallerSha256": "0" * 64,
    "ReleaseDate": "REPLACE-AT-RELEASE",
}


def fetch_schema(name: str) -> dict:
    url = f"{SCHEMA_BASE}/v{VERSION}/{name}"
    with urllib.request.urlopen(url, timeout=30) as response:
        return json.load(response)


def fail(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    sys.exit(1)


documents = [d for d in yaml.safe_load_all(MANIFEST.read_text()) if d]
if len(documents) != 3:
    fail(f"expected 3 documents in {MANIFEST.name}, found {len(documents)}")

by_kind = {d.get("ManifestType"): d for d in documents}
missing = set(KINDS) - set(by_kind)
if missing:
    fail(f"no document with ManifestType {sorted(missing)}")

versions = {d.get("ManifestVersion") for d in documents}
if len(versions) != 1:
    fail(f"the documents disagree on ManifestVersion: {sorted(versions)}")
VERSION = versions.pop()

identifiers = {d.get("PackageIdentifier") for d in documents}
if len(identifiers) != 1:
    fail(f"the documents disagree on PackageIdentifier: {sorted(identifiers)}")

package_versions = {d.get("PackageVersion") for d in documents}
if len(package_versions) != 1:
    fail(f"the documents disagree on PackageVersion: {sorted(package_versions)}")

identifier = identifiers.pop()
print(f"{identifier} {package_versions.pop()}, manifest schema v{VERSION}")

out = HERE / "split"
out.mkdir(exist_ok=True)

failures = 0
for kind, (schema_name, suffix) in KINDS.items():
    document = by_kind[kind]
    path = out / f"{identifier}.{suffix}"
    path.write_text(yaml.safe_dump(document, sort_keys=False, allow_unicode=True))

    try:
        jsonschema.validate(document, fetch_schema(schema_name.format(v=VERSION)))
    except jsonschema.ValidationError as error:
        location = "/".join(str(p) for p in error.absolute_path) or "(root)"
        print(f"  FAIL {path.name}: at {location}: {error.message}")
        failures += 1
    else:
        print(f"  ok   {path.name}")

installer = by_kind["installer"]
for key, placeholder in PLACEHOLDERS.items():
    found = json.dumps(installer)
    if placeholder not in found:
        print(
            f"  note {key} no longer looks like a placeholder — if this is a real "
            f"release, make sure it was computed from the published artefact"
        )

if failures:
    fail(f"{failures} document(s) do not match the winget schema")
print("winget manifest splits into three valid documents")
