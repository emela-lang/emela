#!/usr/bin/env python3
"""Generate the Unicode 17.0.0 property tables from the vendored UCD data.

The data files live in ``crates/emela-codegen-wasm/data/unicode/17.0.0/``. The
generator reads only those files, so it needs no network and no external tools.
Its output is deterministic: the same inputs always produce the same bytes.

    scripts/gen-unicode-tables.py            write the generated table
    scripts/gen-unicode-tables.py --check    fail if the committed table is stale
"""

import sys
from pathlib import Path

VERSION = "17.0.0"

# `Grapheme_Cluster_Break` values, numbered as the runtime expects.
GCB_CLASSES = [
    ("Other", "GCB_OTHER"),
    ("CR", "GCB_CR"),
    ("LF", "GCB_LF"),
    ("Control", "GCB_CONTROL"),
    ("Extend", "GCB_EXTEND"),
    ("ZWJ", "GCB_ZWJ"),
    ("Regional_Indicator", "GCB_REGIONAL_INDICATOR"),
    ("Prepend", "GCB_PREPEND"),
    ("SpacingMark", "GCB_SPACING_MARK"),
    ("L", "GCB_L"),
    ("V", "GCB_V"),
    ("T", "GCB_T"),
    ("LV", "GCB_LV"),
    ("LVT", "GCB_LVT"),
]
GCB_INDEX = {name: i for i, (name, _) in enumerate(GCB_CLASSES)}

# `Indic_Conjunct_Break` values, numbered as the runtime expects.
INCB_CLASSES = [
    ("None", "INCB_NONE"),
    ("Consonant", "INCB_CONSONANT"),
    ("Linker", "INCB_LINKER"),
    ("Extend", "INCB_EXTEND"),
]
INCB_INDEX = {name: i for i, (name, _) in enumerate(INCB_CLASSES)}

# The Hangul syllables are not in the table: LV is index 0 mod 28, LVT any other.
HANGUL_SYLLABLES = (0xAC00, 0xD7A3)

ROOT = Path(__file__).resolve().parent.parent
CRATE_DIR = ROOT / "crates" / "emela-codegen-wasm"
DATA_DIR = CRATE_DIR / "data" / "unicode" / VERSION
OUTPUT = CRATE_DIR / "src" / "emit" / "runtime" / "unicode_tables.rs"


def data_lines(path, handler):
    """Yield the fields of each data line, skipping comments and blank lines."""
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as err:
        raise ValueError(f"{path}: {err}") from err
    for line_no, raw in enumerate(text.splitlines(), 1):
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        fields = [field.strip() for field in line.split(";")]
        try:
            handler(fields)
        except ValueError as err:
            raise ValueError(f"{path}:{line_no}: {err}") from err


def code_range(text):
    """Parse a UCD code point or range (`XXXX` or `XXXX..YYYY`)."""
    if ".." in text:
        first, last = text.split("..", 1)
    else:
        first = last = text

    def parse(value):
        try:
            return int(value, 16)
        except ValueError as err:
            raise ValueError(f"invalid code point {value!r}: {err}") from err

    return parse(first), parse(last)


def merge(ranges, what):
    """Sort ranges and merge the ones of the same class that touch or overlap."""
    ranges.sort()
    out = []
    for first, last, cls in ranges:
        if first > last:
            raise ValueError(f"{what}: empty range {first:04X}..{last:04X}")
        if out:
            previous_first, previous_last, previous_cls = out[-1]
            if first <= previous_last:
                if cls != previous_cls:
                    raise ValueError(
                        f"{what}: overlapping ranges of different classes: "
                        f"{previous_first:04X}..{previous_last:04X} and "
                        f"{first:04X}..{last:04X}"
                    )
                out[-1] = (previous_first, max(previous_last, last), previous_cls)
                continue
            if first == previous_last + 1 and cls == previous_cls:
                out[-1] = (previous_first, last, previous_cls)
                continue
        out.append((first, last, cls))
    return out


def validate_hangul(ranges):
    """Check that the LV/LVT ranges cover the Hangul syllables exactly."""
    if not ranges:
        raise ValueError("the LV/LVT ranges are missing")
    ranges.sort()
    next_code_point = HANGUL_SYLLABLES[0]
    for first, last in ranges:
        if first != next_code_point:
            raise ValueError(
                "the LV/LVT ranges do not cover the Hangul syllables exactly: "
                f"expected {next_code_point:04X}, found {first:04X}"
            )
        if last < first:
            raise ValueError(f"empty Hangul range {first:04X}..{last:04X}")
        next_code_point = last + 1
    if next_code_point != HANGUL_SYLLABLES[1] + 1:
        raise ValueError("the LV/LVT ranges do not cover the Hangul syllables exactly")


def gcb_ranges(data_dir):
    ranges = []
    hangul = []

    def handle(fields):
        if len(fields) != 2:
            raise ValueError(f"unexpected GraphemeBreakProperty line: {fields!r}")
        first, last = code_range(fields[0])
        name = fields[1]
        if name == "Other":
            return
        try:
            cls = GCB_INDEX[name]
        except KeyError:
            raise ValueError(f"unknown Grapheme_Cluster_Break value: {name}") from None
        if name in ("LV", "LVT"):
            hangul.append((first, last))
            return
        ranges.append((first, last, cls))

    data_lines(data_dir / "GraphemeBreakProperty.txt", handle)
    validate_hangul(hangul)
    return merge(ranges, "Grapheme_Cluster_Break")


def extended_pictographic_ranges(data_dir):
    ranges = []

    def handle(fields):
        if len(fields) != 2:
            raise ValueError(f"unexpected emoji-data line: {fields!r}")
        if fields[1] != "Extended_Pictographic":
            return
        first, last = code_range(fields[0])
        ranges.append((first, last, 1))

    data_lines(data_dir / "emoji-data.txt", handle)
    return merge(ranges, "Extended_Pictographic")


def incb_ranges(data_dir):
    ranges = []

    def handle(fields):
        if len(fields) != 3 or fields[1] != "InCB":
            raise ValueError(f"unexpected InCB line: {fields!r}")
        name = fields[2]
        try:
            cls = INCB_INDEX[name]
        except KeyError:
            raise ValueError(f"unknown Indic_Conjunct_Break value: {name}") from None
        # `None` is the default and is never stored in the table.
        if name == "None":
            return
        first, last = code_range(fields[0])
        ranges.append((first, last, cls))

    data_lines(data_dir / "InCB.txt", handle)
    return merge(ranges, "Indic_Conjunct_Break")


def emit_table(name, ranges, doc, class_name):
    out = [f"/// {doc}", f"pub(super) const {name}: &[(u32, u32, u8)] = &["]
    for first, last, cls in ranges:
        out.append(f"    (0x{first:04X}, 0x{last:04X}, {class_name(cls)}),")
    out.append("];")
    return "\n".join(out)


def render(gcb, pictographic, incb):
    out = [
        f"//! Unicode {VERSION} property tables for grapheme cluster segmentation (UAX #29). **Do not edit.**",
        "//!",
        f"//! Generated by `python3 scripts/gen-unicode-tables.py` from `data/unicode/{VERSION}/`.",
        f"//! The data is under the Unicode License V3; see `data/unicode/{VERSION}/LICENSE.txt`.",
        "//! Each entry is `(first, last, class)`, sorted by code point and disjoint. The default",
        "//! `Other` of `Grapheme_Cluster_Break` and the Hangul syllables `LV`/`LVT` (computed from",
        "//! the code point) are not in the tables.",
        "",
        "// The classes of `Grapheme_Cluster_Break` (UAX #29), numbered as the runtime expects.",
    ]
    for cls, (_, name) in enumerate(GCB_CLASSES[1:], 1):
        out.append(f"pub(super) const {name}: u8 = {cls};")
    out.append("")
    out.append("// The classes of `Indic_Conjunct_Break` (UAX #29 GB9c).")
    for cls, (_, name) in enumerate(INCB_CLASSES[1:], 1):
        out.append(f"pub(super) const {name}: u8 = {cls};")
    out.append("")
    out.append(
        emit_table(
            "GCB",
            gcb,
            "`Grapheme_Cluster_Break`, without `Other` and `LV`/`LVT`.",
            lambda cls: GCB_CLASSES[cls][1],
        )
    )
    out.append("")
    out.append(
        emit_table(
            "EXTENDED_PICTOGRAPHIC",
            pictographic,
            "`Extended_Pictographic` from `emoji-data.txt` (GB11). The class is always 1.",
            lambda _cls: "1",
        )
    )
    out.append("")
    out.append(
        emit_table(
            "INCB",
            incb,
            "`Indic_Conjunct_Break` from the InCB extract of `DerivedCoreProperties.txt` (GB9c).",
            lambda cls: INCB_CLASSES[cls][1],
        )
    )
    return "\n".join(out) + "\n"


def generate():
    gcb = gcb_ranges(DATA_DIR)
    pictographic = extended_pictographic_ranges(DATA_DIR)
    incb = incb_ranges(DATA_DIR)
    return render(gcb, pictographic, incb)


def main(argv):
    check = False
    if len(argv) == 2 and argv[1] == "--check":
        check = True
    elif len(argv) != 1:
        print("usage: scripts/gen-unicode-tables.py [--check]", file=sys.stderr)
        return 2

    try:
        text = generate()
    except (OSError, ValueError) as err:
        print(err, file=sys.stderr)
        return 1

    if check:
        try:
            committed = OUTPUT.read_text(encoding="utf-8")
        except OSError as err:
            print(f"cannot read {OUTPUT}: {err}", file=sys.stderr)
            return 1
        if committed == text:
            print("unicode tables are up to date")
            return 0
        print(
            f"{OUTPUT} is stale; run `python3 scripts/gen-unicode-tables.py`",
            file=sys.stderr,
        )
        return 1

    try:
        with open(OUTPUT, "w", encoding="utf-8", newline="\n") as out:
            out.write(text)
    except OSError as err:
        print(f"cannot write {OUTPUT}: {err}", file=sys.stderr)
        return 1
    print(f"wrote {OUTPUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
