#!/usr/bin/env python3
"""Subset the strip UI fonts and give them a legacy `kern` table.

The renderer rasterizes with fontdue, which only reads the TrueType `kern`
table, while Barlow ships its kerning in GPOS. This script subsets each font
to the Latin ranges the strip needs and flattens the GPOS pair adjustments of
the `kern` feature into a format-0 `kern` table.

Requires fontTools (`pacman -S python-fonttools` / `pip install fonttools`).

    python3 tools/build_fonts.py SRC_DIR assets/fonts
"""
import pathlib
import sys

from fontTools import subset
from fontTools.ttLib import TTFont, newTable
from fontTools.ttLib.tables._k_e_r_n import KernTable_format_0

FONTS = [
    "BarlowSemiCondensed-Medium.ttf",
    "BarlowSemiCondensed-SemiBold.ttf",
    "BarlowSemiCondensed-Bold.ttf",
]

UNICODES = (
    list(range(0x20, 0x7F))  # Basic Latin
    + list(range(0xA0, 0x180))  # Latin-1 Supplement, Latin Extended-A
    + [0x2010, 0x2011, 0x2012, 0x2013, 0x2014, 0x2018, 0x2019, 0x201A, 0x201C,
       0x201D, 0x201E, 0x2020, 0x2021, 0x2022, 0x2026, 0x2030, 0x2032, 0x2033,
       0x2039, 0x203A, 0x2044, 0x20AC, 0x2116, 0x2122, 0x2212, 0x2215, 0x2219,
       0x221E, 0x2248, 0x2260, 0x2264, 0x2265]
)


def pair_adjustments(font):
    """Yield (left glyph, right glyph, x-advance adjustment) from GPOS kern."""
    if "GPOS" not in font:
        return
    gpos = font["GPOS"].table
    lookups = set()
    for record in gpos.FeatureList.FeatureRecord:
        if record.FeatureTag == "kern":
            lookups.update(record.Feature.LookupListIndex)
    for index in sorted(lookups):
        lookup = gpos.LookupList.Lookup[index]
        for sub in lookup.SubTable:
            if lookup.LookupType == 9:
                sub = sub.ExtSubTable
            if getattr(sub, "LookupType", 2) != 2 and lookup.LookupType not in (2, 9):
                continue
            coverage = sub.Coverage.glyphs
            if sub.Format == 1:
                for left, pairset in zip(coverage, sub.PairSet):
                    for record in pairset.PairValueRecord:
                        value = getattr(record.Value1, "XAdvance", 0) if record.Value1 else 0
                        if value:
                            yield left, record.SecondGlyph, value
            elif sub.Format == 2:
                class1 = sub.ClassDef1.classDefs
                class2 = sub.ClassDef2.classDefs
                right_glyphs = {}
                for glyph, cls in class2.items():
                    right_glyphs.setdefault(cls, []).append(glyph)
                all_right = set(font.getGlyphOrder())
                right_glyphs[0] = sorted(all_right - set(class2))
                for left in coverage:
                    c1 = class1.get(left, 0)
                    row = sub.Class1Record[c1]
                    for c2, record in enumerate(row.Class2Record):
                        value = getattr(record.Value1, "XAdvance", 0) if record.Value1 else 0
                        if not value:
                            continue
                        for right in right_glyphs.get(c2, []):
                            yield left, right, value


def build(src: pathlib.Path, dst: pathlib.Path) -> None:
    options = subset.Options()
    options.layout_features = ["kern"]
    options.name_IDs = ["*"]
    options.notdef_outline = True
    options.hinting = False
    font = TTFont(src)
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=UNICODES)
    subsetter.subset(font)

    pairs = {}
    for left, right, value in pair_adjustments(font):
        # First matching lookup wins, as in a shaping engine.
        pairs.setdefault((left, right), value)

    kern = newTable("kern")
    kern.version = 0
    table = KernTable_format_0()
    table.version = 0
    table.coverage = 1
    table.format = 0
    table.kernTable = pairs
    kern.kernTables = [table]
    font["kern"] = kern
    for tag in ("GPOS", "GSUB", "GDEF", "DSIG"):
        if tag in font:
            del font[tag]
    font.save(dst)
    print(f"{dst.name}: {len(pairs)} kerning pairs, {dst.stat().st_size} bytes")


def main() -> None:
    src_dir, dst_dir = map(pathlib.Path, sys.argv[1:3])
    dst_dir.mkdir(parents=True, exist_ok=True)
    for name in FONTS:
        build(src_dir / name, dst_dir / name)


if __name__ == "__main__":
    main()
