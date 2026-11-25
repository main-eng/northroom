# Writes the fixture with python-pptx, a writer independent of Northroom's reader. Its default
# template has real placeholders, text styles on the master and a theme.
import sys
from pptx import Presentation
from pptx.util import Inches, Pt
from pptx.dml.color import RGBColor
from pptx.enum.shapes import MSO_SHAPE, MSO_CONNECTOR

deck = Presentation()
title = deck.slides.add_slide(deck.slide_layouts[0])
title.shapes.title.text = "Harbor Monthly"
title.placeholders[1].text = "A coffee subscription"

bullets = deck.slides.add_slide(deck.slide_layouts[1])
bullets.shapes.title.text = "Why a subscription"
body = bullets.placeholders[1].text_frame
body.text = "Steady income"
second = body.add_paragraph(); second.text = "Fresh every month"; second.level = 1
run = second.add_run(); run.text = " guaranteed"; run.font.bold = True; run.font.color.rgb = RGBColor(0x1F, 0x6F, 0x4A)

shapes = deck.slides.add_slide(deck.slide_layouts[6])
box = shapes.shapes.add_shape(MSO_SHAPE.RECTANGLE, Inches(1), Inches(1), Inches(2), Inches(1)); box.text = "Styled box"
pill = shapes.shapes.add_shape(MSO_SHAPE.ROUNDED_RECTANGLE, Inches(4), Inches(1), Inches(2), Inches(1))
pill.fill.solid(); pill.fill.fore_color.rgb = RGBColor(0xF6, 0xF3, 0xEC); pill.line.fill.background()
oval = shapes.shapes.add_shape(MSO_SHAPE.OVAL, Inches(7), Inches(1), Inches(1), Inches(1)); oval.fill.background()
shapes.shapes.add_shape(MSO_SHAPE.RIGHT_ARROW, Inches(1), Inches(3), Inches(2), Inches(1))
chevron = shapes.shapes.add_shape(MSO_SHAPE.CHEVRON, Inches(4), Inches(3), Inches(2), Inches(1)); chevron.text = "Next"
shapes.shapes.add_connector(MSO_CONNECTOR.STRAIGHT, Inches(1), Inches(6), Inches(3), Inches(5))
note = shapes.shapes.add_textbox(Inches(6), Inches(5), Inches(3), Inches(1)); note.text_frame.text = "A text box"
note.text_frame.paragraphs[0].runs[0].font.size = Pt(14)
group = shapes.shapes.add_group_shape()
inner = group.shapes.add_shape(MSO_SHAPE.RECTANGLE, Inches(8), Inches(3), Inches(1), Inches(1))

table_slide = deck.slides.add_slide(deck.slide_layouts[5])
table_slide.shapes.title.text = "Plans"
table = table_slide.shapes.add_table(3, 2, Inches(1), Inches(2), Inches(6), Inches(1.5)).table
for row, (plan, price) in enumerate([("Plan", "Price"), ("Solo", "$19"), ("Pair", "$33")]):
    table.cell(row, 0).text = plan; table.cell(row, 1).text = price
deck.save(sys.argv[1])
