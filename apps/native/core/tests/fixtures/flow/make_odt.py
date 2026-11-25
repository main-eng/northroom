# Writes the fixture with odfpy, a writer independent of Northroom's reader: named and
# automatic styles, a heading, spans, a bulleted and a numbered list, a table with a spanned
# cell, a page break, a picture and the page's size.
import sys, zlib, struct
from odf.opendocument import OpenDocumentText
from odf.style import (Style, TextProperties, ParagraphProperties, PageLayout,
    PageLayoutProperties, MasterPage, TableColumnProperties, TableCellProperties, GraphicProperties, ListLevelProperties)
from odf.text import P, H, Span, List, ListItem, ListStyle, ListLevelStyleBullet, ListLevelStyleNumber, LineBreak
from odf.table import Table, TableColumn, TableRow, TableCell, CoveredTableCell
from odf.draw import Frame, Image

doc = OpenDocumentText()
layout = PageLayout(name="A4")
layout.addElement(PageLayoutProperties(pagewidth="21cm", pageheight="29.7cm", margintop="2cm", marginbottom="2cm", marginleft="2.5cm", marginright="2.5cm"))
doc.automaticstyles.addElement(layout)
doc.masterstyles.addElement(MasterPage(name="Standard", pagelayoutname="A4"))
normal = Style(name="Text", family="paragraph"); normal.addElement(TextProperties(fontsize="11pt"))
normal.addElement(ParagraphProperties(marginbottom="6pt")); doc.styles.addElement(normal)
heading = Style(name="Heading", family="paragraph", parentstylename="Text")
heading.addElement(TextProperties(fontsize="20pt", fontweight="bold", color="#1f6f4a")); doc.styles.addElement(heading)
strong = Style(name="Strong", family="text"); strong.addElement(TextProperties(fontweight="bold")); doc.styles.addElement(strong)
centred = Style(name="P1", family="paragraph", parentstylename="Text"); centred.addElement(ParagraphProperties(textalign="center")); doc.automaticstyles.addElement(centred)
breaks = Style(name="P2", family="paragraph", parentstylename="Text"); breaks.addElement(ParagraphProperties(breakbefore="page")); doc.automaticstyles.addElement(breaks)
wide = Style(name="C1", family="table-column"); wide.addElement(TableColumnProperties(columnwidth="4cm")); doc.automaticstyles.addElement(wide)
shaded = Style(name="Cell1", family="table-cell"); shaded.addElement(TableCellProperties(backgroundcolor="#f6f3ec")); doc.automaticstyles.addElement(shaded)
bullets = ListStyle(name="Bullets")
level = ListLevelStyleBullet(level=1, bulletchar="–"); level.addElement(ListLevelProperties(spacebefore="0.5cm", minlabelwidth="0.5cm")); bullets.addElement(level)
doc.automaticstyles.addElement(bullets)
numbers = ListStyle(name="Numbers")
level = ListLevelStyleNumber(level=1, numformat="1", numsuffix="."); numbers.addElement(level)
doc.automaticstyles.addElement(numbers)

doc.text.addElement(H(outlinelevel=1, stylename=heading, text="Launch plan"))
p = P(stylename=normal); p.addText("Our regulars already buy "); p.addElement(Span(stylename=strong, text="monthly")); p.addText("."); p.addElement(LineBreak()); p.addText("Second line")
doc.text.addElement(p)
doc.text.addElement(P(stylename=centred, text="Centred"))
for style, items in ((bullets, ["Order beans", "Print labels"]), (numbers, ["First", "Second"])):
    listed = List(stylename=style)
    for item in items:
        li = ListItem(); li.addElement(P(stylename=normal, text=item)); listed.addElement(li)
    doc.text.addElement(listed)
table = Table(name="Plans")
table.addElement(TableColumn(stylename=wide, numbercolumnsrepeated=2))
row = TableRow()
cell = TableCell(numbercolumnsspanned=2, stylename=shaded); cell.addElement(P(text="Plans")); row.addElement(cell); row.addElement(CoveredTableCell())
table.addElement(row)
row = TableRow()
for text in ["Solo", "$19"]:
    cell = TableCell(); cell.addElement(P(text=text)); row.addElement(cell)
table.addElement(row)
doc.text.addElement(table)
doc.text.addElement(P(stylename=breaks, text="On the second page"))
def chunk(kind, data): return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
raw = b"".join(b"\x00" + b"\x1f\x6f\x4a" * 4 for _ in range(2))
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 4, 2, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
href = doc.addPicture("Pictures/green.png", "image/png", png)
holder = P(stylename=normal); frame = Frame(width="4cm", height="2cm", anchortype="as-char"); frame.addElement(Image(href=href)); holder.addElement(frame)
doc.text.addElement(holder)
doc.save(sys.argv[1])
