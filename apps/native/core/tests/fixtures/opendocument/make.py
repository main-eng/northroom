# Writes the fixture with odfpy, a writer independent of Northroom's reader.
import sys
from odf.opendocument import OpenDocumentSpreadsheet
from odf.style import Style, TextProperties, TableColumnProperties, ParagraphProperties, TableProperties
from odf.table import Table, TableRow, TableCell, TableColumn
from odf.text import P, Span, S

doc = OpenDocumentSpreadsheet()
bold = Style(name="Head", family="table-cell"); bold.addElement(TextProperties(fontweight="bold")); doc.styles.addElement(bold)
centred = Style(name="HeadCentre", family="table-cell", parentstylename="Head")
centred.addElement(ParagraphProperties(textalign="center")); doc.automaticstyles.addElement(centred)
wide = Style(name="Wide", family="table-column"); wide.addElement(TableColumnProperties(columnwidth="6cm")); doc.automaticstyles.addElement(wide)
gone = Style(name="Gone", family="table"); gone.addElement(TableProperties(display="false")); doc.automaticstyles.addElement(gone)

def text(value, style=None):
    cell = TableCell(valuetype="string", stylename=style) if style else TableCell(valuetype="string")
    cell.addElement(P(text=value)); return cell
def number(value, shown, kind="float"):
    cell = TableCell(valuetype=kind, value=value); cell.addElement(P(text=shown)); return cell

plans = Table(name="Plans")
plans.addElement(TableColumn(stylename="Wide"))
plans.addElement(TableColumn(numbercolumnsrepeated=3, defaultcellstylename="Head"))
row = TableRow()
for name in ["Plan", "Bags a month", "Price", "Margin"]:
    row.addElement(text(name, "HeadCentre" if name == "Plan" else None))
plans.addElement(row)
for plan, bags, price, margin in [("Solo", 1, 19, 0.42), ("Pair", 2, 33, 0.47), ("Household", 4, 61, 0.51)]:
    row = TableRow()
    row.addElement(text(plan))
    row.addElement(number(bags, str(bags)))
    row.addElement(number(price, "$%d.00" % price, "currency"))
    row.addElement(number(margin, "%d%%" % round(margin * 100), "percentage"))
    plans.addElement(row)
plans.addElement(TableRow(numberrowsrepeated=2))
row = TableRow()
note = TableCell(valuetype="string")
p = P(); p.addText("Roasted in "); span = Span(text="Portland"); p.addElement(span); p.addText(","); p.addElement(S(c=2)); p.addText("Maine")
note.addElement(p); row.addElement(note)
row.addElement(TableCell(numbercolumnsrepeated=2))
date = TableCell(valuetype="date", datevalue="2027-01-15"); date.addElement(P(text="15 Jan 2027")); row.addElement(date)
plans.addElement(row)
doc.spreadsheet.addElement(plans)

hidden = Table(name="Working", stylename="Gone")
row = TableRow(); row.addElement(text("not shown")); hidden.addElement(row); doc.spreadsheet.addElement(hidden)

year = Table(name="First year")
row = TableRow(); row.addElement(text("Month")); row.addElement(text("Subscribers")); year.addElement(row)
for month, count in enumerate([40, 65, 90, 120, 150, 185, 220, 250, 285, 320, 350, 390], 1):
    row = TableRow(); row.addElement(number(month, str(month))); row.addElement(number(count, str(count))); year.addElement(row)
doc.spreadsheet.addElement(year)
doc.save(sys.argv[1])
