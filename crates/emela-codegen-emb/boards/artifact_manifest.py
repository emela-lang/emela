"""Read the versioned generated-C contract without importing compiler or IR code."""

import json
from pathlib import Path

MANIFEST_FILE = "emela_artifact.json"
INTERFACES = {"public_header": "emela_embedded.h", "platform_header": "emela_board.h",
              "aliases_header": "emela_embedded_exports.h"}


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate manifest key: {key}")
        result[key] = value
    return result


def load_manifest(directory, require_aliases=False):
    """Return validated role -> ordered paths; never discover extra sources by glob.

    C ABI v2 fixes the three interface header names. Aliases are optional for a C
    library and required by a BSP. All declared paths must be existing files within
    the artifact directory, including when symlinks are present.
    """
    directory = Path(directory).resolve()
    manifest = directory / MANIFEST_FILE
    try:
        data = json.loads(manifest.read_text(encoding="utf-8"), object_pairs_hook=_object)
        if not isinstance(data, dict):
            raise ValueError("expected schema_version, abi_version, and files")
        version = (data.get("schema_version"), data.get("abi_version"))
        if any(type(value) is not int for value in version) or version != (2, 2):
            raise ValueError("unsupported schema/ABI version (expected 2/2; regenerate the artifact)")
        expected = {"schema_version", "abi_version", "files", "metadata"}
        if set(data) != expected:
            raise ValueError("unknown or missing manifest fields")
        metadata = data["metadata"]
        if not isinstance(metadata, dict) or set(metadata) != {"entry", "exports", "services", "external_symbols", "isr_entries", "isr_safe_symbols", "task_capacity"}:
            raise ValueError("invalid ABI 2 metadata")
        for field in ("services", "external_symbols", "isr_entries", "isr_safe_symbols"):
            names = metadata[field]
            if not isinstance(names, list) or any(not isinstance(name, str) or not name for name in names) or len(set(names)) != len(names):
                raise ValueError("invalid metadata " + field)
        if metadata["entry"] is not None and not isinstance(metadata["entry"], str):
            raise ValueError("invalid entry")
        if type(metadata["task_capacity"]) is not int or not 0 <= metadata["task_capacity"] <= 65535:
            raise ValueError("invalid task capacity")
        if not isinstance(metadata["exports"], list) or any(not isinstance(export, dict) or set(export) != {"source_name", "c_symbol", "suspends"} or type(export["suspends"]) is not bool or not isinstance(export["source_name"], str) or not isinstance(export["c_symbol"], str) for export in metadata["exports"]):
            raise ValueError("invalid exports")
        if not isinstance(data["files"], list):
            raise ValueError("files must be a list")
        roles = {role: [] for role in ["source", "header", *INTERFACES]}
        seen = set()
        for entry in data["files"]:
            if not isinstance(entry, dict) or set(entry) != {"path", "role"}:
                raise ValueError("each file needs path and role")
            name, role = entry["path"], entry["role"]
            if (not isinstance(name, str) or any(char in name for char in "\\:\0")
                    or any(part in {"", ".", ".."} for part in name.split("/"))):
                raise ValueError(f"invalid relative path: {name!r}")
            if name in seen:
                raise ValueError(f"duplicate path: {name}")
            seen.add(name)
            if not isinstance(role, str) or role not in roles:
                raise ValueError(f"unknown file role: {role!r}")
            if role in INTERFACES:
                if name != INTERFACES[role] or roles[role]:
                    raise ValueError(f"invalid interface role: {name}")
            elif role == "source":
                if not name.endswith(".c"):
                    raise ValueError("source must have the .c suffix")
            elif not name.endswith(".h") or name in INTERFACES.values():
                raise ValueError(f"invalid header role: {name}")
            path = directory / name
            if directory not in path.resolve().parents or not path.is_file():
                raise ValueError(f"missing or escaping generated file: {name}")
            roles[role].append(path)
        required = ["source", "public_header", "platform_header"]
        if require_aliases:
            required.append("aliases_header")
        if any(not roles[role] for role in required):
            raise ValueError("required file roles: " + ", ".join(required))
        return roles
    except (OSError, ValueError, TypeError) as error:
        raise ValueError(f"invalid generated artifact {manifest}: {error}") from error


def include_directories(inputs):
    """Include the artifact root separately; also support nested generated files."""
    return {path.parent for paths in inputs.values() for path in paths}
