import PhoneLink
import SwiftUI

/// The computer's running sessions, the ones that need the person first (the computer's order).
struct SessionsScreen: View {
    @Environment(Phone.self) private var phone

    var body: some View {
        NavigationStack {
            List(phone.sessions) { session in
                NavigationLink(value: session.id) { SessionRow(session: session) }
            }
            .overlay {
                if phone.state != .connected {
                    Reaching()
                } else if phone.sessions.isEmpty {
                    ContentUnavailableView(
                        "No sessions running", systemImage: "terminal",
                        description: Text("Start one in Northroom on your computer."))
                }
            }
            .navigationTitle(phone.computer?.name ?? "")
            .navigationDestination(for: String.self) { SessionScreen(session: $0) }
            .toolbar {
                Menu {
                    Button("Forget This Computer", role: .destructive) {
                        Task { await phone.forget() }
                    }
                } label: {
                    Label("More", systemImage: "ellipsis.circle")
                }
            }
        }
    }
}

private struct SessionRow: View {
    let session: Session

    var body: some View {
        HStack(spacing: 12) {
            Circle().fill(colour).frame(width: 10, height: 10)
            VStack(alignment: .leading, spacing: 2) {
                Text(session.name)
                Text(detail).font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
            }
        }
    }

    private var detail: String {
        switch session.activity {
        case .needsYou: "Needs you"
        case .finished: "Finished"
        default: session.doing.isEmpty ? session.agent : session.doing
        }
    }

    private var colour: Color {
        switch session.activity {
        case .needsYou: .orange
        case .finished: .green
        case .working: .blue
        case .idle: .gray.opacity(0.4)
        }
    }
}
