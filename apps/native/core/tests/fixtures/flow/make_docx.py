# Writes the fixture with python-docx, a writer independent of Northroom's reader. Its default
# template has styles (Normal, headings, Title), docDefaults, numbering and a theme.
import sys, io
from docx import Document
from docx.shared import Pt, RGBColor, Inches
from docx.enum.text import WD_ALIGN_PARAGRAPH, WD_BREAK

doc = Document()
doc.add_heading("Launch plan", level=0)
doc.add_heading("Why now", level=1)
p = doc.add_paragraph("Our regulars already buy ")
r = p.add_run("monthly"); r.bold = True; r.font.color.rgb = RGBColor(0x1F, 0x6F, 0x4A)
p.add_run(". A subscription makes it easy.")
centred = doc.add_paragraph("Centred and twelve point"); centred.alignment = WD_ALIGN_PARAGRAPH.CENTER
centred.runs[0].font.size = Pt(12)
doc.add_paragraph("Order beans", style="List Bullet")
doc.add_paragraph("Print labels", style="List Bullet")
doc.add_paragraph("First step", style="List Number")
doc.add_paragraph("Second step", style="List Number")
table = doc.add_table(rows=2, cols=3); table.style = "Table Grid"
for i, text in enumerate(["Plan", "Bags", "Price"]): table.cell(0, i).text = text
for i, text in enumerate(["Solo", "1", "$19"]): table.cell(1, i).text = text
brk = doc.add_paragraph("Before the break"); brk.runs[0].add_break(WD_BREAK.PAGE)
doc.add_paragraph("On the second page")
# A 4 by 2 pixel PNG, red.
import zlib, struct
def chunk(kind, data): return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
raw = b"".join(b"\x00" + b"\xff\x00\x00" * 4 for _ in range(2))
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 4, 2, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
doc.add_picture(io.BytesIO(png), width=Inches(2))
doc.save(sys.argv[1])
