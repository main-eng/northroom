import PhoneLink
import SwiftUI

/// The computer's drive, a folder at a time, from the top.
struct FilesScreen: View {
    var body: some View {
        NavigationStack {
            FolderScreen(folder: nil, name: nil)
                .navigationDestination(for: Item.self) { item in
                    if item.isFolder {
                        FolderScreen(folder: item.id, name: item.name)
                    } else {
                        FileScreen(item: item)
                    }
                }
        }
    }
}

/// One folder's files and folders, in the drive's order.
private struct FolderScreen: View {
    /// The folder's id; `nil` is the top of the drive.
    let folder: String?
    /// Its name until the computer has listed it.
    let name: String?
    @Environment(Phone.self) private var phone

    var body: some View {
        let listed = phone.folders[folder ?? Phone.top]
        List(items(of: listed)) { item in
            // A terminal is opened from Sessions, where the running ones are.
            if item.kind == "terminal" {
                ItemRow(item: item)
            } else {
                NavigationLink(value: item) { ItemRow(item: item) }
            }
        }
        .overlay { note(for: listed) }
        .navigationTitle(title(of: listed))
        #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
        #endif
        .task(id: phone.state) { await phone.list(folder: folder) }
        .refreshable { await phone.list(folder: folder) }
    }

    private func items(of listed: Phone.Folder?) -> [Item] {
        if case .listed(_, let items) = listed { items } else { [] }
    }

    private func title(of listed: Phone.Folder?) -> String {
        if case .listed(let listedName, _) = listed { return listedName }
        return name ?? phone.computer?.name ?? ""
    }

    @ViewBuilder private func note(for listed: Phone.Folder?) -> some View {
        if phone.state != .connected {
            Reaching()
        } else {
            switch listed {
            case nil: ProgressView()
            case .gone:
                ContentUnavailableView("This folder is gone", systemImage: "folder.badge.questionmark")
            case .listed(_, let items) where items.isEmpty:
                ContentUnavailableView("Nothing here", systemImage: "folder")
            case .listed: EmptyView()
            }
        }
    }
}

private struct ItemRow: View {
    let item: Item

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: symbol).frame(width: 24).foregroundStyle(.secondary)
            Text(item.name).lineLimit(1)
            Spacer()
            if let size = item.size {
                Text(Int64(size), format: .byteCount(style: .file))
                    .font(.subheadline).foregroundStyle(.secondary)
            }
        }
    }

    private var symbol: String {
        switch item.kind {
        case "folder": "folder"
        case "page": "doc.text"
        case "terminal": "terminal"
        case "image": "photo"
        case "video": "film"
        case "audio": "waveform"
        default: "doc"
        }
    }
}
