import PhoneLink
import SwiftUI

/// A file's sheets: one grid at a time, and the sheets' names to choose from when there are
/// several.
struct TableView: View {
    let file: String
    let sheets: [SheetHead]
    @State private var shown = 0

    var body: some View {
        VStack(spacing: 0) {
            if sheets.indices.contains(shown) {
                SheetGrid(file: file, sheet: shown, head: sheets[shown]).id(shown)
            } else {
                ContentUnavailableView("Nothing here", systemImage: "tablecells")
            }
            if sheets.count > 1 {
                Divider()
                Picker("Sheet", selection: $shown) {
                    ForEach(Array(sheets.enumerated()), id: \.offset) { index, sheet in
                        Text(sheet.name).tag(index)
                    }
                }
                .pickerStyle(.menu)
                .padding(.vertical, 4)
            }
        }
    }
}

/// One sheet: its column names over its rows, each column as wide as the computer shows it.
/// Rows are asked for a window at a time as they come on screen.
private struct SheetGrid: View {
    let file: String
    let sheet: Int
    let head: SheetHead
    @Environment(Phone.self) private var phone

    private static let rowHeight = 30.0
    private static let widthWithoutOne = 96.0

    var body: some View {
        let rows = phone.rows[Phone.SheetKey(file: file, sheet: sheet)] ?? [:]
        GeometryReader { space in
            ScrollView([.horizontal, .vertical]) {
                LazyVStack(alignment: .leading, spacing: 0, pinnedViews: [.sectionHeaders]) {
                    Section {
                        ForEach(0..<head.rows, id: \.self) { row in
                            line(rows[row] ?? [], bold: false)
                                .task { phone.askRows(around: row, sheet: sheet, of: file) }
                        }
                    } header: {
                        line(head.columns.map { TableCell(text: $0, bold: true) }, bold: true)
                            .background(.bar)
                    }
                }
                // A short sheet starts at the top left, not in the middle.
                .frame(
                    minWidth: space.size.width, minHeight: space.size.height, alignment: .topLeading)
            }
        }
        .font(.system(size: 13))
    }

    /// One row: a cell a column, each cut to its column's width.
    private func line(_ cells: [TableCell?], bold: Bool) -> some View {
        HStack(spacing: 0) {
            ForEach(Array(head.columns.indices), id: \.self) { column in
                let cell = cells.indices.contains(column) ? cells[column] : nil
                Text(cell?.text ?? "")
                    .fontWeight(bold || cell?.bold == true ? .semibold : .regular)
                    .lineLimit(1)
                    .padding(.horizontal, 8)
                    .frame(
                        width: head.widths.indices.contains(column)
                            ? head.widths[column] : SheetGrid.widthWithoutOne,
                        height: SheetGrid.rowHeight, alignment: alignment(of: cell))
            }
        }
        .overlay(alignment: .bottom) { Divider() }
    }

    private func alignment(of cell: TableCell?) -> Alignment {
        switch cell?.align {
        case "center": .center
        case "end": .trailing
        default: .leading
        }
    }
}
