"""rustdoc JSON -> a markdown tree `rmem ingest` can read.

Builds a corpus of *reference* documentation, which is what document ingest
turned out to be for: this project's own `docs/` is argument rather than
reference, and nine chunks in thirty produced any fact at all. An API
reference is the opposite shape, and docs.rs will hand you one for any
published crate.

    curl -sL -o arrow-schema.json.zst https://docs.rs/crate/arrow-schema/latest/json
    zstd -d arrow-schema.json.zst
    python scripts/rustdoc-corpus.py ./corpus arrow-schema.json
    rmem ingest ./corpus --dry-run

Two details matter and neither is cosmetic.

Headings inside a doc comment are demoted beneath the item's own heading, so
rustdoc's `# Panics` and `# Examples` stay attached to the item they describe.
Left at depth 1 they reset the heading path, and the text reaches the model
with the wrong subject.

Code fences are left exactly as they are, including the `# ` lines rustdoc
uses to hide doctest setup. The chunker knows about fences; this must not
second-guess it.

"""
import io, json, os, sys, collections

HASH = chr(35)
NL = chr(10)


def children(item):
    """Ids this item owns, so a method can be attributed to its type."""
    inner = item.get("inner") or {}
    out = []
    for key, body in inner.items():
        if not isinstance(body, dict):
            continue
        for field in ("items", "variants", "fields", "impls"):
            v = body.get(field)
            if isinstance(v, list):
                out.extend(x for x in v if isinstance(x, (str, int)))
    return out


def demote(docs, by=2):
    """Shift markdown headings deeper. Code fences are left alone."""
    out, fenced = [], False
    for line in docs.splitlines():
        t = line.lstrip()
        if t.startswith("```") or t.startswith("~~~"):
            fenced = not fenced
            out.append(line)
            continue
        if line.startswith(HASH) and not fenced:
            out.append(HASH * by + line)
        else:
            out.append(line)
    return NL.join(out)


def convert(json_path, outdir):
    d = json.load(io.open(json_path, encoding="utf-8"))
    idx, paths = d["index"], d["paths"]
    crate = os.path.basename(json_path)[: -len(".json")].replace("-", "_")

    # Attribute every item to a path: `paths` where rustdoc gives one, else the
    # owning parent's path plus the item's own name (methods, variants).
    parent = {}
    for pid, item in idx.items():
        for c in children(item):
            parent[str(c)] = pid

    def path_of(iid, depth=0):
        iid = str(iid)
        if iid in paths:
            return list(paths[iid]["path"])
        if depth > 8 or iid not in parent:
            return None
        up = path_of(parent[iid], depth + 1)
        name = (idx.get(iid) or {}).get("name")
        if up is None or not name:
            return None
        return up + [name]

    by_module = collections.defaultdict(list)
    for iid, item in idx.items():
        docs = item.get("docs")
        if not docs or not docs.strip():
            continue
        p = path_of(iid)
        if not p:
            continue
        kind = (paths.get(str(iid)) or {}).get("kind", "item")
        if kind == "module":
            continue                      # module prose becomes the file header
        module = "::".join(p[:-1]) or crate
        by_module[module].append(("::".join(p), kind, docs))

    os.makedirs(outdir, exist_ok=True)
    written = 0
    for module, items in sorted(by_module.items()):
        items.sort()
        name = module.replace("::", ".") + ".md"
        lines = [HASH + " " + module, ""]
        for full, kind, docs in items:
            lines.append(HASH * 2 + " " + full)
            lines.append("")
            lines.append("*" + kind + "*")
            lines.append("")
            lines.append(demote(docs).strip())
            lines.append("")
        io.open(os.path.join(outdir, name), "w", encoding="utf-8", newline=NL).write(
            NL.join(lines) + NL
        )
        written += 1
    return crate, written, sum(len(v) for v in by_module.values())


if __name__ == "__main__":
    outroot = sys.argv[1]
    for jp in sys.argv[2:]:
        crate, files, items = convert(jp, os.path.join(outroot, os.path.basename(jp)[:-5]))
        print(f"{crate:16} {files:4} files  {items:5} documented items")
