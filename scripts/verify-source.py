"""Verify original inventory/bytes and native manifest mappings from the immutable source."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = ROOT / "upstream"
EXPECTED_COMMIT = "dccfc8877dd0e658295cb4b6d29b4a0a1b79baf8"
ORIGINAL_PREFIX = "extensions/github/"


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


def main():
    inventory = read_json(UPSTREAM / "source-blobs.json")
    source = read_json(UPSTREAM / "package.json")
    store = read_json(ROOT / "UPSTREAM.json")
    manifest = read_json(ROOT / "extension.json")
    assert inventory["commit"] == store["commit_sha"] == EXPECTED_COMMIT
    assert manifest["repository"] == "https://github.com/almatkai/github"
    paths = set()
    for row in inventory["files"]:
        rel = row["path"].removeprefix(ORIGINAL_PREFIX)
        paths.add(rel)
        data = (UPSTREAM / rel).read_bytes()
        assert hashlib.sha256(data).hexdigest() == row["sha256"], rel
        assert hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest() == row["gitBlob"], rel
        if rel.startswith("assets/"):
            assert data == (ROOT / "resources" / rel).read_bytes(), rel
    present = {path.relative_to(UPSTREAM).as_posix() for path in UPSTREAM.rglob("*") if path.is_file()}
    assert present == paths | {"source-blobs.json"}
    assert (UPSTREAM / "LICENSE").read_bytes() == (ROOT / "licenses/raycast-extensions-MIT.txt").read_bytes()
    for license in (UPSTREAM / "skills").rglob("LICENSE"):
        copy = ROOT / "licenses" / (license.parent.name + "-LICENSE.txt")
        assert copy.read_bytes() == license.read_bytes()
    assert source["platforms"] == manifest["platforms"] == store["platforms"]
    assert source["author"] == manifest["author"] == "thomaslombart" and source["owner"] == "raycast"
    assert [c["name"] for c in source["commands"]] == [c["name"] for c in manifest["commands"]]
    assert {c["name"] for c in source["commands"]} == {c["name"] for c in store["commands"]}
    assert len(manifest["commands"]) == 25 and len(store["tools"]) == 20
    for orig, port in zip(source["commands"], manifest["commands"]):
        assert {name: port[name] for name in orig} == orig
    for orig, port in zip(source["preferences"], manifest["preferences"]):
        assert port == orig
    assert manifest["replaces"] == ["raycast.github"] and manifest["id"] == "com.tezbar.native.github"
    assert (manifest["schemaVersion"], manifest["runtime"], manifest["execution"], manifest["protocolVersion"]) == (3, "rust", "process", 1)
    print(f"Verified {len(paths)} Git blobs at {EXPECTED_COMMIT}, 25 command IDs, 20 tool IDs, preferences, assets and notices.")


if __name__ == "__main__":
    main()
