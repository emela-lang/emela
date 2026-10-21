#!/usr/bin/env python3
"""Look up the Emela specification (`spec/`) and check proposals (`proposals/`).

Sections are found by their stable names, the `` `[effect.handler]` `` at the end of a heading.

    scripts/spec.py index [PREFIX]     list stable names, numbers and titles
    scripts/spec.py show NAME...       print sections (up to the next heading of the same or higher level)
    scripts/spec.py refs NAME          where a name is referred to (spec, proposals, NOTES.md, code)
    scripts/spec.py proposals [NAME]   list decided proposals and local drafts, or those touching NAME
    scripts/spec.py check              check stable names and proposals for consistency
"""

import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SPEC = os.path.join(ROOT, "spec")
PROPOSALS = os.path.join(ROOT, "proposals")
HEADING = re.compile(r"^(#{1,4}) (.*?)\s*`\[([a-z0-9.\-]+)\]`\s*$")
STATUSES = {"draft", "accepted", "implemented"}


def spec_files():
    for dirpath, dirnames, filenames in os.walk(SPEC):
        dirnames[:] = sorted(d for d in dirnames if not d.startswith("."))
        for f in sorted(filenames):
            if f.endswith(".md"):
                yield os.path.join(dirpath, f)


def sections():
    """Yield (name, title, level, path, start, end) for every named heading. Lines are 0-based."""
    for path in spec_files():
        lines = open(path, encoding="utf-8").read().split("\n")
        heads, fence = [], False
        for i, line in enumerate(lines):
            if line.startswith("```"):
                fence = not fence
            if fence:
                continue
            m = re.match(r"^(#{1,4}) ", line)
            if m:
                h = HEADING.match(line)
                heads.append((i, len(m.group(1)), h.group(3) if h else None, h.group(2) if h else line))
        for k, (i, level, name, title) in enumerate(heads):
            if name is None:
                continue
            end = next((j for j, lv, _, _ in heads[k + 1:] if lv <= level), len(lines))
            yield name, title, level, path, i, end


def rel(path):
    return os.path.relpath(path, ROOT)


def cmd_index(prefix=""):
    for name, title, level, path, start, _ in sections():
        if name.startswith(prefix):
            print(f"{'  ' * (level - 1)}[{name}]  {title}  ({rel(path)}:{start + 1})")


def cmd_show(*names):
    table = {s[0]: s for s in sections()}
    for name in names:
        name = name.strip("[]`")
        if name not in table:
            sys.exit(f"no section named [{name}]; try `scripts/spec.py index {name.split('.')[0]}`")
        _, _, _, path, start, end = table[name]
        lines = open(path, encoding="utf-8").read().split("\n")
        print(f"<!-- {rel(path)}:{start + 1} -->")
        print("\n".join(lines[start:end]).rstrip())
        print()


def cmd_refs(name):
    name = name.strip("[]`")
    pattern = r"\[" + re.escape(name) + r"\]"
    subprocess.run(["rg", "-n", pattern, "spec", "proposals", "crates", "--glob", "*.md", "--glob", "*.rs",
                    "--glob", "*.emel"], cwd=ROOT)


def read_proposal(path):
    text = open(path, encoding="utf-8").read()
    m = re.match(r"^---\n(.*?)\n---\n", text, re.S)
    meta = {}
    if m:
        for line in m.group(1).split("\n"):
            if ":" in line:
                k, v = line.split(":", 1)
                v = v.split("#", 1)[0].strip()
                if v.startswith("[") and v.endswith("]"):
                    v = [x.strip().strip('"\'') for x in v[1:-1].split(",") if x.strip()]
                else:
                    v = v.strip('"\'')
                meta[k.strip()] = v
    return meta, text


def proposals():
    """Yield (file, meta, text, is_local) for proposals in proposals/ and local drafts."""
    for folder, is_local in ((PROPOSALS, False), (os.path.join(PROPOSALS, "drafts"), True)):
        if not os.path.isdir(folder):
            continue
        for f in sorted(os.listdir(folder)):
            if f.endswith(".md") and f not in ("README.md", "TEMPLATE.md"):
                meta, text = read_proposal(os.path.join(folder, f))
                yield ("drafts/" + f if is_local else f), meta, text, is_local


def touched(meta):
    return [t.replace("(new)", "").strip() for t in meta.get("touches", []) or []]


def cmd_proposals(name=None):
    for f, meta, _, is_local in proposals():
        if name and name.strip("[]`") not in touched(meta):
            continue
        state = "local" if is_local else meta.get("status", "?")
        pr = f"#{meta['pr']}" if meta.get("pr") else "-"
        print(f"{pr:6} {state:11}  {meta.get('title', '')}  ({f})")


def cmd_check():
    problems = []
    names = {}
    for name, _, _, path, start, _ in sections():
        if name in names:
            problems.append(f"duplicate name [{name}]: {names[name]} and {rel(path)}:{start + 1}")
        names[name] = f"{rel(path)}:{start + 1}"
    slugs = {}
    for f, meta, text, is_local in proposals():
        slugs[os.path.basename(f)[:-3]] = meta
        status = meta.get("status")
        if status not in STATUSES:
            problems.append(f"{f}: unknown status {status!r}")
        if not is_local and status != "draft" and not re.fullmatch(r"\d+", str(meta.get("pr", ""))):
            problems.append(f"{f}: an {status} proposal needs the number of its pull request in `pr`")
        for t in meta.get("touches", []) or []:
            if "(new)" not in t and t not in names:
                problems.append(f"{f}: touches [{t}], which is not in spec/ (mark a new section with `(new)`)")
        if status in ("accepted", "implemented"):
            open_part = re.search(r"\n## Open questions\n(.*?)(\n## |\Z)", text, re.S)
            if open_part and re.search(r"^- \S", open_part.group(1), re.M):
                problems.append(f"{f}: {status} with open questions left")
    for f, meta, _, _ in proposals():
        for key in ("requires", "related"):
            for r in meta.get(key, []) or []:
                if r not in slugs:
                    problems.append(f"{f}: {key} {r!r} is not the slug of a proposal")
    # The specification is in English; only the glossary carries the Japanese terms used in discussions.
    japanese = re.compile(r"[぀-ヿ㐀-䶿一-鿿]")
    for path in spec_files():
        if os.path.basename(path) == "glossary.md":
            continue
        for i, line in enumerate(open(path, encoding="utf-8").read().split("\n"), 1):
            if japanese.search(line):
                problems.append(f"{rel(path)}:{i}: Japanese text in the specification")
    heads = {n for n in names if "." not in n}
    for path in spec_files():
        text = open(path, encoding="utf-8").read()
        for m in re.finditer(r"`\[([a-z][a-z0-9.\-]*)\]`", text):
            ref = m.group(1)
            # A name without a dot that is not a chapter head is a TOML table (`[external]`), not a section.
            if ref not in names and ("." in ref or ref in heads):
                line = text[: m.start()].count("\n") + 1
                problems.append(f"{rel(path)}:{line}: refers to [{ref}], which does not exist")
    for p in problems:
        print(p)
    print(f"{len(names)} sections, {len(slugs)} proposals, {len(problems)} problems")
    sys.exit(1 if problems else 0)


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    cmd, args = sys.argv[1], sys.argv[2:]
    {"index": cmd_index, "show": cmd_show, "refs": cmd_refs, "proposals": cmd_proposals,
     "check": cmd_check}.get(cmd, lambda *_: sys.exit(__doc__))(*args)


if __name__ == "__main__":
    main()
