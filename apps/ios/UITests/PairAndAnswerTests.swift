import XCTest

/// The whole app against a running Northroom: pair, open a session, answer it.
///
/// Needs a computer showing Connect Phone… with one session running. Pass its pairing link as
/// `TEST_RUNNER_NR_PAIR_LINK` to `xcodebuild test`; without it the test is skipped.
final class PairAndAnswerTests: XCTestCase {
    @MainActor
    func testPairingOpensASessionAndTypesIntoIt() throws {
        guard let link = ProcessInfo.processInfo.environment["NR_PAIR_LINK"],
            let url = URL(string: link)
        else { throw XCTSkip("no pairing link given") }
        let app = XCUIApplication()
        app.launch()
        keep("1 scan")

        XCUIDevice.shared.system.open(url)
        let open = XCUIApplication(bundleIdentifier: "com.apple.springboard").buttons["Open"]
        if open.waitForExistence(timeout: 5) { open.tap() }

        let session = app.cells.firstMatch
        XCTAssertTrue(session.waitForExistence(timeout: 30), "the computer's sessions are listed")
        keep("2 sessions")
        session.tap()

        let message = app.textFields["Message"]
        XCTAssertTrue(message.waitForExistence(timeout: 10))
        sleep(3)
        keep("3 session")
        message.tap()
        message.typeText("echo typed on the phone\n")
        sleep(3)
        keep("4 answered")
    }

    @MainActor
    private func keep(_ name: String) {
        let picture = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        picture.name = name
        picture.lifetime = .keepAlways
        add(picture)
    }
}
