# Writes the fixture with xlwt, an Excel 97-2003 writer independent of Northroom's reader: a
# hidden and two visible sheets, shared strings (one long enough to cross a CONTINUE record),
# integers and decimals (stored as RK and NUMBER), a date, a percentage, booleans, an error, a
# formula (saved without a result, as xlwt can't calculate), bold text and a column width.
import sys, datetime
import xlwt
book = xlwt.Workbook()
bold = xlwt.easyxf("font: bold on")
money = xlwt.easyxf(num_format_str="$#,##0.00")
date = xlwt.easyxf(num_format_str="yyyy-mm-dd")
share = xlwt.easyxf(num_format_str="0%")
plans = book.add_sheet("Plans")
for column, heading in enumerate(["Plan", "Bags", "Price", "Margin"]):
    plans.write(0, column, heading, bold)
for row, (plan, bags, price, margin) in enumerate([("Solo", 1, 19, 0.42), ("Pair", 2, 33.5, 0.47)], 1):
    plans.write(row, 0, plan)
    plans.write(row, 1, bags)
    plans.write(row, 2, price, money)
    plans.write(row, 3, margin, share)
plans.write(3, 0, "Launch")
plans.write(3, 1, datetime.date(2026, 4, 1), date)
plans.write(4, 0, True)
plans.write(4, 1, xlwt.Formula("B2+B3"))
plans.col(0).width = 256 * 20
hidden = book.add_sheet("Working")
hidden.write(0, 0, "not shown")
hidden.visibility = 1
notes = book.add_sheet("Notes")
notes.write(0, 0, "Long " + "x" * 9000)
for row in range(1, 900):
    notes.write(row, 0, "note %d" % row)
book.save(sys.argv[1])
