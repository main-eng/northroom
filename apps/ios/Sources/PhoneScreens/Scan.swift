import SwiftUI

#if canImport(VisionKit) && os(iOS)
    import VisionKit
#endif

/// The first screen: the camera, looking for the pairing code on the computer.
struct ScanScreen: View {
    @Environment(Phone.self) private var phone

    var body: some View {
        ZStack(alignment: .bottom) {
            Color.black.ignoresSafeArea()
            camera.ignoresSafeArea()
            VStack(spacing: 8) {
                if phone.pairing {
                    ProgressView("Pairing")
                } else {
                    Text("On your computer, choose Northroom > Connect Phone…")
                        .font(.headline)
                    Text(phone.notice ?? "Then point this phone at the code.")
                        .foregroundStyle(.secondary)
                }
            }
            .multilineTextAlignment(.center)
            .padding()
            .frame(maxWidth: .infinity)
            .background(.regularMaterial)
        }
    }

    @ViewBuilder private var camera: some View {
        #if canImport(VisionKit) && os(iOS)
            if DataScannerViewController.isSupported {
                CodeCamera { text in Task { await phone.pair(link: text) } }
            }
        #endif
    }
}

#if canImport(VisionKit) && os(iOS)
    /// The system's code scanner, reporting each QR code's text.
    private struct CodeCamera: UIViewControllerRepresentable {
        let found: (String) -> Void

        func makeCoordinator() -> Reader { Reader(found: found) }

        func makeUIViewController(context: Context) -> DataScannerViewController {
            let scanner = DataScannerViewController(
                recognizedDataTypes: [.barcode(symbologies: [.qr])], isHighlightingEnabled: true)
            scanner.delegate = context.coordinator
            return scanner
        }

        func updateUIViewController(_ scanner: DataScannerViewController, context: Context) {
            // Fails until the person allows the camera; the next update tries again.
            try? scanner.startScanning()
        }

        final class Reader: NSObject, DataScannerViewControllerDelegate {
            let found: (String) -> Void

            init(found: @escaping (String) -> Void) { self.found = found }

            func dataScanner(
                _ dataScanner: DataScannerViewController, didAdd addedItems: [RecognizedItem],
                allItems: [RecognizedItem]
            ) {
                for case .barcode(let code) in addedItems {
                    if let text = code.payloadStringValue { found(text) }
                }
            }
        }
    }
#endif
