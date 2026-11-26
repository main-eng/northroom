import PhoneLink
import SwiftUI

/// The whole app: the scan screen until a computer is paired, then its sessions and its files.
public struct Root: View {
    @State private var phone = Phone()
    @Environment(\.scenePhase) private var phase

    public init() {}

    public var body: some View {
        Group {
            if phone.computer == nil {
                ScanScreen()
            } else {
                TabView {
                    SessionsScreen().tabItem { Label("Sessions", systemImage: "terminal") }
                    FilesScreen().tabItem { Label("Files", systemImage: "folder") }
                }
            }
        }
        .environment(phone)
        // The system camera opens a scanned pairing link here too.
        .onOpenURL { url in Task { await phone.pair(link: url.absoluteString) } }
        .task(id: phase) {
            switch phase {
            case .active: await phone.wake()
            case .background: await phone.rest()
            default: break
            }
        }
    }
}

/// Shown over a screen while the computer is not connected.
struct Reaching: View {
    @Environment(Phone.self) private var phone

    var body: some View {
        let name = phone.computer?.name ?? "the computer"
        if phone.state == .connecting {
            ProgressView("Connecting to \(name)")
        } else if phone.state == .unreachable {
            ContentUnavailableView {
                Label("Can't reach \(name)", systemImage: "wifi.slash")
            } description: {
                Text("Northroom has to be open on it.")
            } actions: {
                Button("Try Again") { Task { await phone.wake() } }
            }
        }
    }
}
