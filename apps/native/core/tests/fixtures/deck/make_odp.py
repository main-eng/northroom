# Writes the fixture with odfpy, a writer independent of Northroom's reader: a master page with
# a background and its own shape, a title and outline placeholder in text frames, styled
# shapes, a line, a picture and a slide with its own background.
import sys, zlib, struct
from odf.opendocument import OpenDocumentPresentation
from odf.style import (Style, MasterPage, PageLayout, PageLayoutProperties, TextProperties,
    GraphicProperties, ParagraphProperties, DrawingPageProperties)
from odf.text import P, Span, List, ListItem, ListStyle, ListLevelStyleBullet
from odf.draw import Page, Frame, TextBox, Image, CustomShape, EnhancedGeometry, Line, Rect
from odf.namespaces import PRESENTATIONNS

def placed(element, kind, placeholder=False):
    element.setAttrNS(PRESENTATIONNS, "class", kind)
    if placeholder:
        element.setAttrNS(PRESENTATIONNS, "placeholder", "true")
    return element

doc = OpenDocumentPresentation()
layout = PageLayout(name="Wide"); layout.addElement(PageLayoutProperties(pagewidth="28cm", pageheight="15.75cm"))
doc.automaticstyles.addElement(layout)
cream = Style(name="Cream", family="drawing-page"); cream.addElement(DrawingPageProperties(fill="solid", fillcolor="#f6f3ec"))
doc.automaticstyles.addElement(cream)
green_page = Style(name="Green", family="drawing-page"); green_page.addElement(DrawingPageProperties(fill="solid", fillcolor="#1f6f4a"))
doc.automaticstyles.addElement(green_page)
default = Style(name="standard", family="graphic"); default.addElement(TextProperties(fontsize="18pt", color="#141412"))
doc.styles.addElement(default)
title = Style(name="Title", family="presentation"); title.addElement(TextProperties(fontsize="40pt", fontweight="bold"))
title.addElement(GraphicProperties(fill="none", stroke="none", textareaverticalalign="middle")); doc.styles.addElement(title)
outline = Style(name="Outline", family="presentation"); outline.addElement(TextProperties(fontsize="24pt"))
outline.addElement(GraphicProperties(fill="none", stroke="none")); doc.styles.addElement(outline)
band = Style(name="Band", family="graphic", parentstylename="standard"); band.addElement(GraphicProperties(fill="solid", fillcolor="#1f6f4a", stroke="none"))
doc.automaticstyles.addElement(band)
pill = Style(name="Pill", family="graphic", parentstylename="standard"); pill.addElement(GraphicProperties(fill="solid", fillcolor="#e9dfc9", stroke="solid", strokecolor="#1f6f4a", strokewidth="0.05cm"))
pill.addElement(TextProperties(color="#1f6f4a")); doc.automaticstyles.addElement(pill)
rule = Style(name="Rule", family="graphic"); rule.addElement(GraphicProperties(stroke="solid", strokecolor="#d9d2c3"))
doc.automaticstyles.addElement(rule)
light = Style(name="Light", family="paragraph"); light.addElement(TextProperties(color="#f6f3ec")); light.addElement(ParagraphProperties(textalign="center"))
doc.automaticstyles.addElement(light)
dashes = ListStyle(name="Dashes"); dashes.addElement(ListLevelStyleBullet(level=1, bulletchar="–")); doc.automaticstyles.addElement(dashes)

master = MasterPage(name="Plain", pagelayoutname=layout, stylename=cream)
strip = Rect(x="0cm", y="0cm", width="28cm", height="0.5cm", stylename=band); master.addElement(strip)
slot = placed(Frame(stylename=title, x="1cm", y="1cm", width="26cm", height="2cm"), "title", True)
slot.addElement(TextBox()); master.addElement(slot)
doc.masterstyles.addElement(master)

one = Page(name="One", masterpagename=master)
frame = placed(Frame(stylename=title, x="1.5cm", y="1.5cm", width="25cm", height="3cm"), "title")
box = TextBox(); box.addElement(P(text="Harbor Monthly")); frame.addElement(box); one.addElement(frame)
frame = placed(Frame(stylename=outline, x="1.5cm", y="5cm", width="25cm", height="8cm"), "outline")
box = TextBox(); listed = List(stylename=dashes)
for text in ["Roasted every Monday", "Delivered by bike"]:
    item = ListItem(); item.addElement(P(text=text)); listed.addElement(item)
box.addElement(listed); frame.addElement(box); one.addElement(frame)
doc.presentation.addElement(one)

two = Page(name="Two", masterpagename=master, stylename=green_page)
shape = CustomShape(stylename=pill, x="2cm", y="2cm", width="8cm", height="3cm")
shape.addElement(EnhancedGeometry(type="round-rectangle")); shape.addElement(P(text="Solo · $19")); two.addElement(shape)
oval = CustomShape(stylename=pill, x="12cm", y="2cm", width="3cm", height="3cm"); oval.addElement(EnhancedGeometry(type="ellipse")); two.addElement(oval)
two.addElement(Line(stylename=rule, x1="2cm", y1="7cm", x2="26cm", y2="7cm"))
def chunk(kind, data): return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
raw = b"".join(b"\x00" + b"\xe9\xdf\xc9" * 4 for _ in range(2))
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 4, 2, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
href = doc.addPicture("Pictures/sand.png", "image/png", png)
picture = Frame(x="17cm", y="2cm", width="8cm", height="4cm"); picture.addElement(Image(href=href)); two.addElement(picture)
words = Frame(x="2cm", y="9cm", width="24cm", height="2cm"); box = TextBox(); box.addElement(P(stylename=light, text="Spring 2026")); words.addElement(box); two.addElement(words)
doc.presentation.addElement(two)
doc.save(sys.argv[1])
