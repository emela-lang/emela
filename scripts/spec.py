#!/usr/bin/env python3
"""Look up the Emela specification (`spec/`) and check proposals (`proposals/`).

Sections are found by their stable names, the `` `[effect.handler]` `` at the end of a heading.

    scripts/spec.py index [PREFIX]     list stable names, numbers and titles
    scripts/spec.py show NAME...       print sections (up to the next heading of the same or higher level)
    scripts/spec.py refs NAME          where a name is referred to (spec, proposals, NOTES.md, code)
    scripts/spec.py proposals [NAME]   list proposals (adopted, rejected and local drafts), or those touching NAME
    scripts/spec.py check              check stable names and proposals for consistency
    scripts/spec.py ready SLUG         check that proposals/SLUG.md can be adopted as it stands
    scripts/spec.py accept SLUG FILE DATE PR
                                       record the adoption of a merged proposal, with the reasons in FILE
    scripts/spec.py reject PROPOSAL FILE DATE PR
                                       record the rejection of the proposal in the file PROPOSAL
                                       (as it stood when its pull request was closed) in proposals/rejected/
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
JAPANESE = re.compile(r"[぀-ヿ㐀-䶿一-鿿]")


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
    """The path relative to the repository root, separated with `/` on every OS."""
    return os.path.relpath(path, ROOT).replace(os.sep, "/")


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
    subprocess.run(["rg", "-n", "--path-separator", "/", pattern, "spec", "proposals", "crates",
                    "--glob", "*.md", "--glob", "*.rs", "--glob", "*.emel"], cwd=ROOT)


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
    """Yield (file, meta, text, is_local) for the proposals in proposals/, the rejected ones in
    proposals/rejected/ and the local drafts in proposals/drafts/."""
    for sub, is_local in (("", False), ("rejected/", False), ("drafts/", True)):
        folder = os.path.join(PROPOSALS, sub)
        if not os.path.isdir(folder):
            continue
        for f in sorted(os.listdir(folder)):
            if f.endswith(".md") and f not in ("README.md", "TEMPLATE.md"):
                meta, text = read_proposal(os.path.join(folder, f))
                yield sub + f, meta, text, is_local


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
        rejected = f.startswith("rejected/")
        if rejected:
            if status != "rejected":
                problems.append(f"{f}: a proposal in rejected/ has `status: rejected`, not {status!r}")
        elif status not in STATUSES:
            problems.append(f"{f}: unknown status {status!r}")
        # A proposal is merged with `status: draft`; the record of its adoption fills in `pr`.
        if not is_local and status != "draft" and not re.fullmatch(r"\d+", str(meta.get("pr", ""))):
            problems.append(f"{f}: an {status} proposal needs the number of its pull request in `pr`")
        # A rejected proposal is kept as it was; the sections it named may never have existed.
        for t in [] if rejected else meta.get("touches", []) or []:
            if "(new)" not in t and t not in names:
                problems.append(f"{f}: touches [{t}], which is not in spec/ (mark a new section with `(new)`)")
        if status in ("accepted", "implemented"):
            open_part = re.search(r"\n## Open questions\n(.*?)(\n## |\Z)", text, re.S)
            if open_part and re.search(r"^- \S", open_part.group(1), re.M):
                problems.append(f"{f}: {status} with open questions left")
    committed = {os.path.basename(f)[:-3] for f, _, _, is_local in proposals() if not is_local}
    for f, meta, _, is_local in proposals():
        for key in ("requires", "related"):
            for r in meta.get(key, []) or []:
                # A proposal in the repository can refer only to proposals in the repository: local
                # drafts are not committed, so CI would not see them.
                known = slugs if is_local else committed
                if r not in known:
                    where = "a proposal" if is_local else "a proposal in proposals/ (local drafts are not committed)"
                    problems.append(f"{f}: {key} {r!r} is not the slug of {where}")
    # The specification is in English; only the glossary carries the Japanese terms used in discussions.
    for path in spec_files():
        if os.path.basename(path) == "glossary.md":
            continue
        for i, line in enumerate(open(path, encoding="utf-8").read().split("\n"), 1):
            if JAPANESE.search(line):
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


def section_body(text, heading):
    """The span of the text under `## heading`, up to the next `## ` heading or the end."""
    m = re.search(r"^## " + re.escape(heading) + r"\n(.*?)(?=^## |\Z)", text, re.S | re.M)
    return (m.start(1), m.end(1)) if m else None


def set_field(text, key, value):
    """Set `key: value` in the front matter, adding the line before its end when it is missing."""
    line = f"{key}: {value}" if value != "" else f"{key}:"
    if re.search(rf"^{key}:.*$", text, re.M):
        return re.sub(rf"^{key}:.*$", lambda _: line, text, count=1, flags=re.M)
    end = text.index("\n---\n", 4)
    return text[:end] + "\n" + line + text[end:]


def set_section(text, heading, body):
    span = section_body(text, heading)
    if span:
        tail = "\n" if span[1] < len(text) else ""
        inner = "\n" + body + "\n" if body else ""
        return text[:span[0]] + inner + tail + text[span[1]:]
    return text.rstrip("\n") + f"\n\n## {heading}\n\n" + body + "\n"


def ready_problems(path):
    """Why the proposal in `path` cannot be adopted as it stands (proposals/README.md, "Decision")."""
    meta, text = read_proposal(path)
    problems = []
    if meta.get("status") != "draft":
        problems.append(f"it is {meta.get('status')!r}, not draft")
    span = section_body(text, "Open questions")
    if span and re.search(r"^- \S", text[span[0]:span[1]], re.M):
        problems.append("it still has open questions; settle them, or move them out of the proposal")
    lines = [i for i, line in enumerate(text.split("\n"), 1) if JAPANESE.search(line)]
    if lines:
        problems.append(f"it has Japanese text on lines {', '.join(map(str, lines))}; "
                        "an adopted proposal is in English")
    return problems


def cmd_ready(slug):
    path = os.path.join(PROPOSALS, slug + ".md")
    if not os.path.isfile(path):
        sys.exit(f"no proposal proposals/{slug}.md")
    problems = ready_problems(path)
    for p in problems:
        print(f"proposals/{slug}.md: {p}")
    sys.exit(1 if problems else 0)


def read_decision(decision_file):
    decision = open(decision_file, encoding="utf-8").read().strip()
    if not decision:
        sys.exit("the decision is empty: write the reasons")
    return decision


def cmd_accept(slug, decision_file, date, pr):
    """Record the author's adoption of a merged proposal: the front matter, "Decision" and
    `spec/history.md`. Its pull request already changed `spec/`."""
    path = os.path.join(PROPOSALS, slug + ".md")
    if not os.path.isfile(path):
        sys.exit(f"no proposal proposals/{slug}.md")
    problems = ready_problems(path)
    if problems:
        sys.exit(f"proposals/{slug}.md: " + "; ".join(problems))
    decision = read_decision(decision_file)
    meta, text = read_proposal(path)

    text = set_field(text, "status", "accepted")
    text = set_field(text, "pr", pr)
    text = set_field(text, "decided", date)
    text = set_section(text, "Open questions", "")
    text = set_section(text, "Decision", decision)
    open(path, "w", encoding="utf-8").write(text)

    span = section_body(text, "Summary")
    summary = " ".join(text[span[0]:span[1]].split()) if span else ""
    names = ", ".join(f"`[{t}]`" for t in touched(meta))
    entry = (f"## {date} ({meta.get('title', slug)})\n\n"
             f"- {names}: {summary} Adopted in the proposal `{slug}` (#{pr}).\n\n")
    history = os.path.join(SPEC, "history.md")
    old = open(history, encoding="utf-8").read()
    first = re.search(r"^## ", old, re.M)
    at = first.start() if first else len(old)
    open(history, "w", encoding="utf-8").write(old[:at] + entry + old[at:])
    print(f"recorded the adoption of proposals/{slug}.md on {date}")


def cmd_reject(source, decision_file, date, pr):
    """Record the author's rejection: the proposal as it stood when its pull request was closed,
    with the reasons, in proposals/rejected/. The specification does not change."""
    decision = read_decision(decision_file)
    text = open(source, encoding="utf-8").read()
    if not re.match(r"^---\n.*?\n---\n", text, re.S):
        sys.exit(f"{source} has no front matter")
    slug = os.path.basename(source)[:-3]
    folder = os.path.join(PROPOSALS, "rejected")
    os.makedirs(folder, exist_ok=True)
    # The same slug may be proposed and rejected again; each rejection keeps its own file.
    path = os.path.join(folder, slug + ".md")
    if os.path.exists(path):
        path = os.path.join(folder, f"{slug}-{pr}.md")
    text = set_field(text, "status", "rejected")
    text = set_field(text, "pr", pr)
    text = set_field(text, "decided", date)
    text = set_section(text, "Decision", decision)
    open(path, "w", encoding="utf-8").write(text)
    print(f"recorded the rejection in {rel(path)}")


def main():
    # The spec is UTF-8 and has characters that legacy code pages lack (`∖` in `[type.rules]`).
    # When stdout is not a console (a pipe or a file), Python encodes it with the locale's code
    # page (cp932 on a Japanese Windows), which fails on those characters.
    sys.stdout.reconfigure(encoding="utf-8")
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    cmd, args = sys.argv[1], sys.argv[2:]
    {"index": cmd_index, "show": cmd_show, "refs": cmd_refs, "proposals": cmd_proposals,
     "check": cmd_check, "ready": cmd_ready, "accept": cmd_accept, "reject": cmd_reject}.get(cmd, lambda *_: sys.exit(__doc__))(*args)


if __name__ == "__main__":
    main()
