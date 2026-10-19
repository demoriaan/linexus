#!/usr/bin/env python3
"""Pack the Lucide icon set into one compact text file for the renderer.

Input is an unpacked `lucide-static` npm package (`npm pack lucide-static`).
Output is one line per icon: `name<TAB>comma separated tags<TAB>svg body`,
where the body holds the inner elements of the 24x24 stroke icon. The
renderer and Studio both read this file, so the icon picker and the strip
always offer the same set.

    python3 -I tools/gen_icons.py path/to/lucide-static/package assets/icons
"""
import html
import re
import json
import pathlib
import shutil
import sys


def element(tag, attrs):
    parts = "".join(f' {key}="{html.escape(str(value), quote=True)}"' for key, value in attrs.items())
    return f"<{tag}{parts}/>"


def main():
    package, out_dir = map(pathlib.Path, sys.argv[1:3])
    nodes = json.loads((package / "icon-nodes.json").read_text())
    tags = json.loads((package / "tags.json").read_text())
    version = json.loads((package / "package.json").read_text())["version"]
    out_dir.mkdir(parents=True, exist_ok=True)
    lines = [f"# lucide-static {version}, ISC License (see LICENSE)"]
    bodies = {name: "".join(element(tag, attrs) for tag, attrs in node) for name, node in nodes.items()}
    # Deprecated names ship as extra SVG files; keep them as aliases so
    # configs written against older Lucide releases keep their icons.
    for svg in (package / "icons").glob("*.svg"):
        name = svg.stem
        if name in bodies:
            continue
        match = re.search(r"<svg[^>]*>(.*)</svg>", svg.read_text(), re.S)
        if match:
            bodies[name] = re.sub(r"\s*\n\s*", "", match.group(1)).replace(" />", "/>")
    for name in sorted(bodies):
        words = ",".join(t.replace(",", " ").replace("\t", " ") for t in tags.get(name, []))
        lines.append(f"{name}\t{words}\t{bodies[name]}")
    (out_dir / "lucide.txt").write_text("\n".join(lines) + "\n")
    shutil.copyfile(package / "LICENSE", out_dir / "LICENSE")
    print(f"{len(bodies)} icons, {(out_dir / 'lucide.txt').stat().st_size} bytes")


if __name__ == "__main__":
    main()
