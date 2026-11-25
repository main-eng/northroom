# Writes small fonts with fontTools, a writer independent of Northroom's reader: a TrueType face
# (quadratic outlines), a CFF face (cubic outlines), a collection of both, and a Mac suitcase
# holding the TrueType face as its one `sfnt` resource. Each has glyphs for "A", "B" and space.
import struct, sys
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.pens.t2CharStringPen import T2CharStringPen
from fontTools.ttLib import TTCollection, TTFont

out = sys.argv[1]
ORDER = [".notdef", "A", "B", "space"]
CMAP = {ord("A"): "A", ord("B"): "B", ord(" "): "space"}
METRICS = {name: (600, 0) for name in ORDER}

def draw_box(pen, curved):
    pen.moveTo((100, 0))
    pen.lineTo((500, 0))
    if curved:
        # Quadratic in TrueType; the CFF pen writes it as a cubic curve.
        pen.qCurveTo((500, 700), (100, 700))
    else:
        pen.lineTo((500, 700))
        pen.lineTo((100, 700))
    pen.closePath()

def names(family):
    return {"familyName": family, "styleName": "Regular", "fullName": family + " Regular", "version": "Version 1.000"}

def truetype(family):
    builder = FontBuilder(1000, isTTF=True)
    builder.setupGlyphOrder(ORDER)
    builder.setupCharacterMap(CMAP)
    glyphs = {}
    for name in ORDER:
        pen = TTGlyphPen(None)
        if name in ("A", "B"):
            draw_box(pen, curved=(name == "B"))
        glyphs[name] = pen.glyph()
    builder.setupGlyf(glyphs)
    builder.setupHorizontalMetrics(METRICS)
    builder.setupHorizontalHeader(ascent=800, descent=-200)
    builder.setupNameTable(names(family))
    builder.setupOS2(sTypoAscender=800, usWinAscent=800, usWinDescent=200)
    builder.setupPost()
    return builder.font

def cff(family):
    builder = FontBuilder(1000, isTTF=False)
    builder.setupGlyphOrder(ORDER)
    builder.setupCharacterMap(CMAP)
    charstrings = {}
    for name in ORDER:
        pen = T2CharStringPen(600, None)
        if name in ("A", "B"):
            draw_box(pen, curved=(name == "B"))
        charstrings[name] = pen.getCharString()
    builder.setupCFF(family.replace(" ", ""), {"FullName": family}, charstrings, {})
    builder.setupHorizontalMetrics(METRICS)
    builder.setupHorizontalHeader(ascent=800, descent=-200)
    builder.setupNameTable(names(family))
    builder.setupOS2(sTypoAscender=800, usWinAscent=800, usWinDescent=200)
    builder.setupPost()
    return builder.font

truetype("Box Sans").save(out + "/Box Sans.ttf")
cff("Box Serif").save(out + "/Box Serif.otf")
collection = TTCollection()
collection.fonts = [TTFont(out + "/Box Sans.ttf"), TTFont(out + "/Box Serif.otf")]
collection.save(out + "/Box Family.ttc")

# A resource fork in a data fork: header, the font as resource data, then the map with one type
# (`sfnt`) holding one resource.
sfnt = open(out + "/Box Sans.ttf", "rb").read()
data = struct.pack(">I", len(sfnt)) + sfnt
type_list = struct.pack(">H", 0) + b"sfnt" + struct.pack(">HH", 0, 2 + 8)
reference = struct.pack(">hHI", 128, 0xFFFF, 0) + b"\x00" * 4
resource_map = b"\x00" * 24 + struct.pack(">HH", 28, 28 + len(type_list) + len(reference)) + type_list + reference
data_offset, map_offset = 256, 256 + len(data)
header = struct.pack(">IIII", data_offset, map_offset, len(data), len(resource_map))
fork = header + b"\x00" * (256 - len(header)) + data + resource_map
open(out + "/Box Sans.dfont", "wb").write(fork)
