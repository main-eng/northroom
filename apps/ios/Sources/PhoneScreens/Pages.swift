import CoreText
import PhoneLink
import SwiftUI

extension Image {
    /// A decoded picture as an image to draw.
    init(picture: PlatformPicture) {
        #if canImport(UIKit)
            self.init(uiImage: picture)
        #else
            self.init(nsImage: picture)
        #endif
    }
}

/// A file's pages, one under another, as wide as the screen; a pinch looks closer. Each page is
/// asked for when it comes on screen and let go when it leaves.
struct PagesView: View {
    let file: String
    let pageWidth: Double
    let pageHeight: Double
    let count: Int
    @State private var zoom = 1.0
    @GestureState private var pinch = 1.0

    /// The space between two pages.
    private static let gap = 8.0
    private static let zoomMost = 6.0

    var body: some View {
        GeometryReader { space in
            let width = Double(space.size.width)
            let scale = zoom * pinch
            ScrollView([.horizontal, .vertical]) {
                LazyVStack(spacing: PagesView.gap) {
                    ForEach(0..<count, id: \.self) { page in
                        PageView(file: file, page: page, width: width, scale: scale)
                            .frame(
                                width: width * scale,
                                height: width * pageHeight / max(pageWidth, 1) * scale)
                    }
                }
            }
            .gesture(
                MagnifyGesture()
                    .updating($pinch) { value, pinch, _ in pinch = value.magnification }
                    .onEnded { zoom = min(max(zoom * $0.magnification, 1), PagesView.zoomMost) }
            )
        }
        .background(Color.gray.opacity(0.2))
    }
}

/// One page: what the computer says it draws, laid out `width` points wide and drawn `scale`
/// times larger.
private struct PageView: View {
    let file: String
    let page: Int
    let width: Double
    let scale: Double
    @Environment(Phone.self) private var phone

    var body: some View {
        let marks = phone.marks[Phone.PageKey(file: file, page: page)]
        Canvas { context, _ in
            context.scaleBy(x: scale, y: scale)
            for mark in marks ?? [] { draw(mark, in: &context) }
        }
        .background(.white)
        .overlay { if marks == nil { ProgressView() } }
        .task(id: width) { phone.askPage(page, of: file, width: width) }
        .onChange(of: marks) { _, marks in
            for picture in (marks ?? []).compactMap(\.picture) {
                phone.askPicture(picture, of: file)
            }
        }
        .onDisappear { phone.discardPage(page, of: file) }
    }

    private func draw(_ mark: Mark, in context: inout GraphicsContext) {
        let colour = Color(hex: mark.colour) ?? .clear
        switch mark.mark {
        case "fill":
            guard let shape = Shaped(mark) else { return }
            if shape.isLine {
                context.stroke(shape.path, with: .color(colour), lineWidth: 1)
            } else {
                context.fill(shape.path, with: .color(colour))
            }
        case "outline":
            guard let shape = Shaped(mark) else { return }
            context.stroke(shape.path, with: .color(colour), lineWidth: mark.width ?? 1)
        case "picture": drawPicture(mark, in: &context)
        case "words": drawWords(mark, colour, in: &context)
        case "figure":
            context.fill(FigurePath.path(of: mark.path ?? ""), with: .color(colour))
        default: break
        }
    }

    /// A picture fills its frame once `crop` is cut from its sides.
    private func drawPicture(_ mark: Mark, in context: inout GraphicsContext) {
        guard let frame = CGRect(frame: mark.frame), let number = mark.picture,
            let picture = phone.pictures[Phone.PictureKey(file: file, picture: number)]
        else { return }
        let crop = mark.crop ?? [0, 0, 0, 0]
        let kept = (max(1 - crop[0] - crop[2], 0.01), max(1 - crop[1] - crop[3], 0.01))
        let whole = CGSize(width: frame.width / kept.0, height: frame.height / kept.1)
        let full = CGRect(
            x: frame.minX - whole.width * crop[0], y: frame.minY - whole.height * crop[1],
            width: whole.width, height: whole.height)
        var clipped = context
        clipped.clip(to: Path(frame))
        clipped.draw(Image(picture: picture), in: full)
    }

    /// A run of text on the baseline its line shares.
    private func drawWords(_ mark: Mark, _ colour: Color, in context: inout GraphicsContext) {
        guard let text = mark.text, let x = mark.x, let top = mark.top, let height = mark.height,
            let size = mark.size
        else { return }
        let mono = mark.mono == true
        var font = Faces.font(size: size, weight: mark.weight ?? 400, mono: mono)
        if mark.slanted == true { font = font.italic() }
        let resolved = context.resolve(Text(text).font(font).foregroundStyle(colour))
        let (ascent, descent) = Faces.rise(size: mark.lineSize ?? size, mono: mono)
        let baseline = top + (height - (ascent + descent)) / 2 + ascent
        let own = resolved.firstBaseline(in: CGSize(width: 100_000, height: 100_000))
        context.draw(resolved, at: CGPoint(x: x, y: baseline - own), anchor: .topLeading)
    }
}

extension CGRect {
    /// A rectangle from a mark's left, top, width and height.
    init?(frame: [Double]?) {
        guard let frame, frame.count == 4 else { return nil }
        self.init(x: frame[0], y: frame[1], width: frame[2], height: frame[3])
    }
}

/// The faces the computer sets pages in: the app carries the same two font files the desktop
/// does, so a line is as wide here as where it was laid out. Without them (a build that is not
/// the app) the system's faces stand in.
enum Faces {
    private static let sans = "Inter"
    private static let mono = "Geist Mono"

    /// Whether the app's font files were found and made known to the system, once.
    private static let carried: Bool = {
        let files = [nil, "Fonts"].flatMap {
            Bundle.main.urls(forResourcesWithExtension: "woff2", subdirectory: $0) ?? []
        }
        for file in files {
            CTFontManagerRegisterFontsForURL(file as CFURL, .process, nil)
        }
        return !files.isEmpty
    }()

    static func font(size: Double, weight: Double, mono: Bool) -> Font {
        guard carried else {
            return .system(
                size: size, weight: Weight.named(weight), design: mono ? .monospaced : .default)
        }
        return .custom(mono ? Faces.mono : sans, fixedSize: size).weight(Weight.named(weight))
    }

    /// How far text `size` large rises above and falls below its baseline.
    static func rise(size: Double, mono: Bool) -> (ascent: Double, descent: Double) {
        let face =
            carried
            ? CTFontCreateWithName((mono ? Faces.mono : sans) as CFString, size, nil)
            : CTFontCreateUIFontForLanguage(.system, size, nil)
        guard let face else { return (size, 0) }
        return (CTFontGetAscent(face), CTFontGetDescent(face))
    }
}

/// The weight nearest a CSS weight number.
enum Weight {
    static func named(_ number: Double) -> Font.Weight {
        switch number {
        case ..<350: .light
        case ..<450: .regular
        case ..<550: .medium
        case ..<650: .semibold
        default: .bold
        }
    }
}

/// A mark's form as a path in its frame.
struct Shaped {
    let path: Path
    let isLine: Bool

    init?(_ mark: Mark) {
        guard let frame = CGRect(frame: mark.frame) else { return nil }
        switch mark.form {
        case "rounded":
            path = Path(roundedRect: frame, cornerRadius: mark.radius ?? 0)
            isLine = false
        case "oval":
            path = Path(ellipseIn: frame)
            isLine = false
        case "line", "rising-line":
            let rising = mark.form == "rising-line"
            var line = Path()
            line.move(to: CGPoint(x: frame.minX, y: rising ? frame.maxY : frame.minY))
            line.addLine(to: CGPoint(x: frame.maxX, y: rising ? frame.minY : frame.maxY))
            path = line
            isLine = true
        default:
            path = Path(frame)
            isLine = false
        }
    }
}

/// Reads a figure's path as the computer writes it: `M`, `L`, `Q`, `C` and `Z`, each followed
/// by its numbers.
enum FigurePath {
    static func path(of text: String) -> Path {
        var path = Path()
        var numbers: [Double] = []
        var command: Character?
        var number = ""
        func endNumber() {
            if let value = Double(number) { numbers.append(value) }
            number = ""
        }
        func endCommand() {
            endNumber()
            apply(command, numbers, to: &path)
            numbers = []
        }
        for character in text {
            if character.isLetter {
                endCommand()
                command = character
            } else if character == " " {
                endNumber()
            } else {
                number.append(character)
            }
        }
        endCommand()
        return path
    }

    private static func apply(_ command: Character?, _ n: [Double], to path: inout Path) {
        switch (command, n.count) {
        case ("M", 2): path.move(to: CGPoint(x: n[0], y: n[1]))
        case ("L", 2): path.addLine(to: CGPoint(x: n[0], y: n[1]))
        case ("Q", 4):
            path.addQuadCurve(to: CGPoint(x: n[2], y: n[3]), control: CGPoint(x: n[0], y: n[1]))
        case ("C", 6):
            path.addCurve(
                to: CGPoint(x: n[4], y: n[5]), control1: CGPoint(x: n[0], y: n[1]),
                control2: CGPoint(x: n[2], y: n[3]))
        case ("Z", _): path.closeSubpath()
        default: break
        }
    }
}
