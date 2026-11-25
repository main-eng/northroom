# Writes a small PowerPoint 97 file: a compound file with a "Current User" stream and a
# "PowerPoint Document" stream of records. Slide 1 keeps its text in its shapes, as PowerPoint
# 2007 and later save; slide 2 points its shapes into the slide list's text, as PowerPoint 2003
# saves. The reader was also checked by hand against Apache POI's basic_test_ppt_file.ppt and
# SampleShow.ppt, which are not kept here.
import struct, sys

def record(kind, data=b"", instance=0, container=False):
    header = (instance << 4) | (0x0F if container else 0)
    return struct.pack("<HHI", header, kind, len(data)) + data

def text_header(role):
    return record(0x0F9F, struct.pack("<I", role))

def text_bytes(text):
    return record(0x0FA8, text.encode("latin-1"))

def text_style(length, runs):
    # One paragraph run with no fields, then character runs: (count, masks, fields).
    data = struct.pack("<IHI", length + 1, 0, 0)
    for count, masks, fields in runs:
        data += struct.pack("<II", count, masks) + fields
    return record(0x0FA1, data)

def shape(kind, anchor, text_records, fill=None):
    properties = b""
    count = 0
    if fill is not None:
        properties += struct.pack("<HI", 0x0181, fill) + struct.pack("<HI", 0x01BF, 0x00100010)
        count = 2
    parts = record(0xF00A, struct.pack("<II", 1, 0), instance=kind)
    if count:
        parts += record(0xF00B, properties, instance=count)
    parts += record(0xF010, struct.pack("<hhhh", *anchor))
    if text_records:
        parts += record(0xF00D, b"".join(text_records), container=True)
    return record(0xF004, parts, container=True)

def slide(shapes):
    drawing = record(0xF002, record(0xF003, b"".join(shapes), container=True), container=True)
    return record(0x03EE, record(0x040C, drawing, container=True), container=True)

title = "Harbor Monthly"
styled = "Roasted every Monday"
slide_one = slide([
    shape(1, (1342, 432, 5328, 2268), [text_header(6), text_bytes(title)]),
    shape(1, (2448, 864, 4896, 3552), [
        text_header(5), text_bytes(styled),
        # Bold and 20 points and green for the first 7 characters, plain after.
        text_style(len(styled), [(7, 0x00060001, struct.pack("<HH", 1, 20) + bytes([31, 111, 74, 0xFE])), (14, 0, b"")]),
    ]),
    shape(3, (3600, 4800, 5400, 4200), [], fill=0x004A6F1F),
])
slide_two = slide([
    shape(1, (173, 288, 5472, 893), [record(0x0F9E, struct.pack("<i", 0))]),
    shape(1, (1008, 288, 5472, 3859), [record(0x0F9E, struct.pack("<i", 1))]),
])

# The stream: the document, the two slides, the persist directory, the user edit.
document_atom = record(0x03E9, struct.pack("<ii", 5760, 4320) + b"\x00" * 32)
slide_list = record(0x0FF0, b"".join([
    record(0x03F3, struct.pack("<IIiII", 2, 0, 0, 256, 0)),
    record(0x03F3, struct.pack("<IIiII", 3, 0, 0, 257, 0)),
    text_header(0), text_bytes("Plans"),
    text_header(1), text_bytes("Solo\rPair\rHousehold"),
]), container=True)
document = record(0x03E8, document_atom + slide_list, container=True)
stream = bytearray(document)
places = {1: 0}
places[2] = len(stream); stream += slide_one
places[3] = len(stream); stream += slide_two
directory_at = len(stream)
stream += record(0x1772, struct.pack("<I", (3 << 20) | 1) + struct.pack("<III", places[1], places[2], places[3]))
edit_at = len(stream)
stream += record(0x0FF5, struct.pack("<IHBBIII", 0, 0, 0, 3, 0, directory_at, 1))
current_user = record(0x0FF6, struct.pack("<IIIHH", 20, 0xE391C05F, edit_at, 8, 0x3F4))
# A stream under 4096 bytes would belong in small sectors; this one is kept in regular ones, so
# it is padded to that size.
stream += b"\x00" * max(0, 4096 - len(stream))

# A compound file of 512-byte sectors: the table, the directory, the small-stream store, the
# small table and the streams, all in regular sectors except the small "Current User".
SECTOR = 512
def pad(data):
    return bytes(data) + b"\x00" * (-len(data) % SECTOR)
big = pad(stream)
small = pad(current_user[:])  # one 64-byte small sector holds it; the store is one sector
sectors_big = len(big) // SECTOR
# Sector layout: 0 table, 1 directory, 2 small table, 3 small store, 4.. the document stream.
END, FREE, TABLE = 0xFFFFFFFE, 0xFFFFFFFF, 0xFFFFFFFD
table = [TABLE, END, END, END] + [4 + i + 1 for i in range(sectors_big - 1)] + [END]
table += [FREE] * (SECTOR // 4 - len(table))
assert len(table) == SECTOR // 4
def entry(name, kind, start, size, child=0xFFFFFFFF, right=0xFFFFFFFF):
    units = name.encode("utf-16-le") + b"\x00\x00"
    data = units.ljust(64, b"\x00") + struct.pack("<HBB", len(units), kind, 1)
    data += struct.pack("<III", 0xFFFFFFFF, right, child) + b"\x00" * 36 + struct.pack("<II", start, size) + b"\x00" * 4
    return data
small_sectors = (len(current_user) + 63) // 64
directory = entry("Root Entry", 5, 3, small_sectors * 64, child=1)
directory += entry("PowerPoint Document", 2, 4, len(stream), right=2)
directory += entry("Current User", 2, 0, len(current_user))
directory += b"\x00" * (SECTOR - len(directory))
small_table = [i + 1 for i in range(small_sectors - 1)] + [END]
small_table += [FREE] * (SECTOR // 4 - len(small_table))
header = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1" + b"\x00" * 16 + struct.pack("<HHHHH", 0x3E, 3, 0xFFFE, 9, 6)
header += b"\x00" * 6 + struct.pack("<IIIIIIIII", 0, 1, 1, 0, 4096, 2, 1, END, 0)
header += struct.pack("<I", 0) + struct.pack("<I", FREE) * 108
assert len(header) == SECTOR
out = header + struct.pack("<128I", *table) + directory + struct.pack("<128I", *small_table) + pad(current_user) + big
open(sys.argv[1], "wb").write(out)
