import AVKit
import PhoneLink
import SwiftUI

/// One file. A kind the computer shows as pages (a PDF, a document, slides) is drawn from what
/// the computer says each page draws; any other is brought to this device and shown by its kind.
struct FileScreen: View {
    let item: Item
    @Environment(Phone.self) private var phone

    var body: some View {
        Group {
            switch phone.paged[item.id] {
            case .pages(let width, let height, let count):
                PagesView(file: item.id, pageWidth: width, pageHeight: height, count: count)
            case .notPaged: fetched
            case nil: ProgressView()
            }
        }
        .navigationTitle(item.name)
        #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar(.hidden, for: .tabBar)
        #endif
        .task { phone.askPages(of: item.id) }
        .onChange(of: phone.paged[item.id]) { _, paged in
            if paged == .notPaged { phone.fetch(item) }
        }
        .onDisappear {
            phone.discard(item.id)
            phone.discardPages(of: item.id)
        }
    }

    @ViewBuilder private var fetched: some View {
        switch phone.fetches[item.id] {
        case .ready(let url): FileView(item: item, url: url)
        case .failed:
            ContentUnavailableView("Can't read this file", systemImage: "exclamationmark.triangle")
        case .fetching(let received, let size):
            ProgressView(value: Double(received), total: Double(max(size, received, 1)))
                .padding(40)
        case nil: ProgressView()
        }
    }
}

/// How a kind of file is shown.
private enum Showing {
    case picture, film, page, text, unknown

    /// Plain files this size or smaller are read as text when they are text.
    private static let textMostBytes = 2 * 1024 * 1024

    init(_ item: Item, at url: URL) {
        switch item.kind {
        case "image": self = .picture
        case "video", "audio": self = .film
        case "page": self = .page
        default:
            let small = (item.size ?? 0) <= Showing.textMostBytes
            self = small && (try? String(contentsOf: url, encoding: .utf8)) != nil ? .text : .unknown
        }
    }
}

private struct FileView: View {
    let item: Item
    let url: URL

    var body: some View {
        switch Showing(item, at: url) {
        case .picture: Picture(url: url)
        case .film: Film(url: url)
        case .page: PageText(text: (try? String(contentsOf: url, encoding: .utf8)) ?? "")
        case .text: PlainText(text: (try? String(contentsOf: url, encoding: .utf8)) ?? "")
        case .unknown:
            ContentUnavailableView(
                "No view for this kind yet", systemImage: "doc",
                description: Text(item.fileName))
        }
    }
}

/// A picture, fitted to the screen; a pinch looks closer.
private struct Picture: View {
    let url: URL
    @State private var zoom = 1.0
    @GestureState private var pinch = 1.0

    var body: some View {
        ScrollView([.horizontal, .vertical]) {
            picture?
                .resizable()
                .scaledToFit()
                .containerRelativeFrame([.horizontal, .vertical]) { length, _ in
                    length * zoom * pinch
                }
        }
        .gesture(
            MagnifyGesture()
                .updating($pinch) { value, pinch, _ in pinch = value.magnification }
                .onEnded { zoom = min(max(zoom * $0.magnification, 1), 8) }
        )
    }

    private var picture: Image? {
        PlatformPicture(contentsOfFile: url.path).map(Image.init(picture:))
    }
}

/// A film or a sound, played by the system's player.
private struct Film: View {
    let url: URL
    @State private var player: AVPlayer?

    var body: some View {
        VideoPlayer(player: player)
            .onAppear { player = AVPlayer(url: url) }
            .onDisappear { player?.pause() }
    }
}

/// Text as it is written, in the terminal's type.
private struct PlainText: View {
    let text: String

    var body: some View {
        ScrollView {
            Text(text)
                .font(.system(.footnote, design: .monospaced))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding()
        }
    }
}

/// A page (Markdown) to read: headings, lists, quotes, code and paragraphs, a block a line.
private struct PageText: View {
    let text: String

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                ForEach(Array(PageBlock.blocks(of: text).enumerated()), id: \.offset) { _, block in
                    block.view
                }
            }
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding()
        }
    }
}

/// One block of a page.
enum PageBlock: Equatable {
    case heading(level: Int, String)
    case bullet(String)
    /// A numbered item, with its number as written ("2.").
    case numbered(String, String)
    case quote(String)
    case code(String)
    case rule
    case paragraph(String)

    /// The page's blocks, top to bottom. Lines of one paragraph join; a fence holds code as
    /// written.
    static func blocks(of text: String) -> [PageBlock] {
        var blocks: [PageBlock] = []
        var paragraph: [String] = []
        var code: [String]?
        func endParagraph() {
            if !paragraph.isEmpty { blocks.append(.paragraph(paragraph.joined(separator: " "))) }
            paragraph = []
        }
        for line in text.components(separatedBy: "\n") {
            if line.hasPrefix("```") {
                endParagraph()
                if let fenced = code { blocks.append(.code(fenced.joined(separator: "\n"))) }
                code = code == nil ? [] : nil
            } else if code != nil {
                code?.append(line)
            } else if let block = single(line) {
                endParagraph()
                blocks.append(block)
            } else if line.trimmingCharacters(in: .whitespaces).isEmpty {
                endParagraph()
            } else {
                paragraph.append(line.trimmingCharacters(in: .whitespaces))
            }
        }
        endParagraph()
        if let fenced = code { blocks.append(.code(fenced.joined(separator: "\n"))) }
        return blocks
    }

    /// The block `line` is by itself, when it is one.
    private static func single(_ line: String) -> PageBlock? {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        let marks = trimmed.prefix { $0 == "#" }.count
        if (1...6).contains(marks), trimmed.dropFirst(marks).hasPrefix(" ") {
            return .heading(level: marks, String(trimmed.dropFirst(marks + 1)))
        }
        if trimmed.hasPrefix("- ") || trimmed.hasPrefix("* ") {
            return .bullet(String(trimmed.dropFirst(2)))
        }
        let digits = trimmed.prefix { $0.isNumber }.count
        if digits > 0, trimmed.dropFirst(digits).hasPrefix(". ") {
            return .numbered(String(trimmed.prefix(digits + 1)), String(trimmed.dropFirst(digits + 2)))
        }
        if trimmed.hasPrefix("> ") { return .quote(String(trimmed.dropFirst(2))) }
        if trimmed == "---" || trimmed == "***" { return .rule }
        return nil
    }

    /// Bold, italic, code and links inside a block.
    private static func inline(_ text: String) -> AttributedString {
        let options = AttributedString.MarkdownParsingOptions(
            interpretedSyntax: .inlineOnlyPreservingWhitespace)
        return (try? AttributedString(markdown: text, options: options)) ?? AttributedString(text)
    }

    /// A list item: its mark, then its text.
    private static func item(_ mark: String, _ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(mark).monospacedDigit()
            Text(inline(text))
        }
    }

    @ViewBuilder var view: some View {
        switch self {
        case .heading(let level, let text):
            Text(PageBlock.inline(text))
                .font(level == 1 ? .title.bold() : level == 2 ? .title2.bold() : .headline)
                .padding(.top, 6)
        case .bullet(let text): PageBlock.item("•", text)
        case .numbered(let number, let text): PageBlock.item(number, text)
        case .quote(let text):
            Text(PageBlock.inline(text)).foregroundStyle(.secondary).padding(.leading, 12)
        case .code(let text):
            ScrollView(.horizontal) {
                Text(text).font(.system(.footnote, design: .monospaced)).padding(10)
            }
            .background(.quaternary, in: RoundedRectangle(cornerRadius: 6))
        case .rule: Divider()
        case .paragraph(let text): Text(PageBlock.inline(text))
        }
    }
}
