import PhoneLink
import SwiftUI

/// The computer's running sessions, the ones that need the person first (the computer's order).
struct SessionsScreen: View {
    @Environment(Phone.self) private var phone
    /// The session on screen, when one is.
    @State private var opened: [String] = []

    var body: some View {
        NavigationStack(path: $opened) {
            List(phone.sessions) { session in
                NavigationLink(value: session.id) { SessionRow(session: session) }
            }
            .overlay {
                if phone.state != .connected {
                    Reaching()
                } else if phone.sessions.isEmpty {
                    ContentUnavailableView(
                        "No sessions running", systemImage: "terminal",
                        description: Text("Start one with the plus."))
                }
            }
            .navigationTitle(phone.computer?.name ?? "")
            .navigationDestination(for: String.self) { SessionScreen(session: $0) }
            .onChange(of: phone.started) { _, started in
                guard let started else { return }
                opened = [started]
                phone.openedStarted()
            }
            .toolbar {
                Button {
                    Task { await phone.start() }
                } label: {
                    Label("New Terminal", systemImage: "plus")
                }
                .disabled(phone.state != .connected)
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
        default: session.doing.isEmpty ? (session.agent.isEmpty ? "Shell" : session.agent) : session.doing
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
