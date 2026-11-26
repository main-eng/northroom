import CoreText
import PhoneLink
import SwiftUI

/// One session: its terminal, fitted to this screen, and a place to answer it.
struct SessionScreen: View {
    let session: String
    @Environment(Phone.self) private var phone
    @Environment(\.dismiss) private var dismiss
    @State private var text = ""
    @FocusState private var typing: Bool
    /// The lines the finger's drag has scrolled so far.
    @State private var linesDragged = 0

    var body: some View {
        VStack(spacing: 0) {
            GeometryReader { space in
                let grid = Cell.grid(in: space.size)
                ZStack(alignment: .topLeading) {
                    Color(hex: phone.screen?.ground) ?? Color.black
                    if let screen = phone.screen { TerminalText(screen: screen) }
                    if phone.state != .connected {
                        Reaching().frame(maxWidth: .infinity, maxHeight: .infinity)
                            .background(.regularMaterial)
                    }
                }
                .clipped()
                .contentShape(Rectangle())
                .onTapGesture { typing = true }
                .gesture(scrolling)
                // Waits out the keyboard's slide, then asks for the grid that fits.
                .task(id: grid) {
                    try? await Task.sleep(for: .milliseconds(200))
                    guard !Task.isCancelled else { return }
                    await phone.watch(session: session, columns: grid.columns, lines: grid.lines)
                }
            }
            Divider()
            keys
            answer
        }
        .navigationTitle(phone.sessions.first { $0.id == session }?.name ?? "")
        #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
        #endif
        .onDisappear { Task { await phone.leave() } }
        .onChange(of: phone.gone) { _, gone in if gone { dismiss() } }
    }

    /// Dragging moves through earlier output a line at a time, as the finger crosses each one.
    private var scrolling: some Gesture {
        DragGesture(minimumDistance: Cell.size.height)
            .onChanged { drag in
                let lines = Int(drag.translation.height / Cell.size.height)
                let step = lines - linesDragged
                guard step != 0 else { return }
                linesDragged = lines
                Task { await phone.scroll(lines: step) }
            }
            .onEnded { _ in linesDragged = 0 }
    }

    /// The keys a phone's keyboard lacks.
    private var keys: some View {
        HStack(spacing: 0) {
            key("esc", .escape)
            key("tab", .tab)
            key(symbol: "arrow.left", .left)
            key(symbol: "arrow.up", .up)
            key(symbol: "arrow.down", .down)
            key(symbol: "arrow.right", .right)
            key("stop", .interrupt).tint(.red)
        }
        .font(.callout)
        .padding(.top, 4)
    }

    private func key(_ name: String, _ pressed: PressedKey) -> some View {
        Button(name) { Task { await phone.press(pressed) } }
            .frame(maxWidth: .infinity, minHeight: 36)
    }

    private func key(symbol: String, _ pressed: PressedKey) -> some View {
        Button {
            Task { await phone.press(pressed) }
        } label: {
            Image(systemName: symbol).frame(maxWidth: .infinity, minHeight: 36)
        }
        .accessibilityLabel(pressed.rawValue)
    }

    private var answer: some View {
        HStack(spacing: 8) {
            TextField("Message", text: $text)
                .textFieldStyle(.roundedBorder)
                .focused($typing)
                .onSubmit(send)
            Button(action: send) {
                Image(systemName: "arrow.up.circle.fill").font(.title)
            }
            .accessibilityLabel(text.isEmpty ? "Enter" : "Send")
        }
        .padding(.horizontal)
        .padding(.vertical, 8)
    }

    /// Sends what was typed and Enter; with nothing typed, just Enter.
    private func send() {
        let typed = text
        text = ""
        // The keyboard stays up, so the terminal is not refitted after every answer.
        typing = true
        Task { await phone.send(typed) }
    }
}

/// The size of one character of the terminal's type.
enum Cell {
    static let fontName = "Menlo"
    static let fontSize: CGFloat = 12
    static let size: CGSize = {
        let font = CTFontCreateWithName(fontName as CFString, fontSize, nil)
        var letter: UniChar = 77
        var glyph = CGGlyph()
        CTFontGetGlyphsForCharacters(font, &letter, &glyph, 1)
        let width = CTFontGetAdvancesForGlyphs(font, .horizontal, &glyph, nil, 1)
        let height = CTFontGetAscent(font) + CTFontGetDescent(font) + CTFontGetLeading(font)
        return CGSize(width: width, height: height.rounded(.up))
    }()

    struct Grid: Equatable {
        var columns: Int
        var lines: Int
    }

    /// The whole characters that fit in `space`.
    static func grid(in space: CGSize) -> Grid {
        Grid(columns: Int(space.width / size.width), lines: Int(space.height / size.height))
    }
}

/// A terminal's screen, drawn as rows of text.
struct TerminalText: View {
    let screen: PhoneLink.Screen

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(screen.rows.enumerated()), id: \.offset) { _, runs in
                Text(line(runs))
                    .lineLimit(1)
                    .fixedSize()
                    .frame(height: Cell.size.height, alignment: .leading)
            }
        }
        .font(.custom(Cell.fontName, fixedSize: Cell.fontSize))
        .foregroundStyle(Color(hex: screen.ink) ?? .white)
        .overlay(alignment: .topLeading) { cursor }
    }

    @ViewBuilder private var cursor: some View {
        if let at = screen.cursor, at.count == 2 {
            Rectangle()
                .fill((Color(hex: screen.ink) ?? .white).opacity(0.5))
                .frame(width: Cell.size.width, height: Cell.size.height)
                .offset(x: CGFloat(at[1]) * Cell.size.width, y: CGFloat(at[0]) * Cell.size.height)
        }
    }

    /// One row: each run in its look, at its column.
    private func line(_ runs: [Run]) -> AttributedString {
        var line = AttributedString()
        var column = 0
        for run in runs {
            if run.column > column {
                line += AttributedString(String(repeating: " ", count: run.column - column))
            }
            line += styled(run)
            column = max(column, run.column) + run.text.count
        }
        return line
    }

    private func styled(_ run: Run) -> AttributedString {
        var text = AttributedString(run.text)
        var ink = Color(hex: run.ink)
        if run.dim == true { ink = (ink ?? Color(hex: screen.ink) ?? .white).opacity(0.6) }
        if let ink { text.foregroundColor = ink }
        if let ground = Color(hex: run.ground) { text.backgroundColor = ground }
        var font = Font.custom(Cell.fontName, fixedSize: Cell.fontSize)
        if run.bold == true { font = font.bold() }
        if run.italic == true { font = font.italic() }
        text.font = font
        if run.underline == true { text.underlineStyle = .single }
        if run.strikeout == true { text.strikethroughStyle = .single }
        return text
    }
}

extension Color {
    /// A colour from `#rrggbb`.
    init?(hex: String?) {
        guard let hex, hex.hasPrefix("#"), hex.utf8.count == 7,
            let value = UInt32(hex.dropFirst(), radix: 16)
        else { return nil }
        self.init(
            red: Double(value >> 16 & 0xff) / 255, green: Double(value >> 8 & 0xff) / 255,
            blue: Double(value & 0xff) / 255)
    }
}
