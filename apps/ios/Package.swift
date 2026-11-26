// swift-tools-version: 5.9
// The phone's side of the phone link (ADR-0016, docs/phone-link.md).
//
// `PhoneLink` is everything but the screens: reading a pairing link, the messages, and the
// connection. `phone-link-check` is a device with no screen, for checking a running Northroom
// from a terminal. The app's screens build on `PhoneLink`.
import PackageDescription

let package = Package(
    name: "NorthroomPhone",
    // What iroh's Swift package asks for.
    platforms: [.iOS("17.5"), .macOS("14.5")],
    products: [
        .library(name: "PhoneLink", targets: ["PhoneLink"]),
        .library(name: "PhoneScreens", targets: ["PhoneScreens"]),
    ],
    dependencies: [
        // Connections dialled by key, the same library the computer's side uses
        // (`crates/northroom-link`).
        .package(url: "https://github.com/n0-computer/iroh-swift", from: "1.2.0")
    ],
    targets: [
        .target(
            name: "PhoneLink",
            dependencies: [.product(name: "IrohLib", package: "iroh-swift")]
        ),
        // The app's screens.
        .target(name: "PhoneScreens", dependencies: ["PhoneLink"]),
        .executableTarget(name: "phone-link-check", dependencies: ["PhoneLink"]),
        .testTarget(name: "PhoneLinkTests", dependencies: ["PhoneLink"]),
    ]
)
