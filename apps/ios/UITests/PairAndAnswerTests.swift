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

        XCTAssertTrue(app.buttons["New Terminal"].waitForExistence(timeout: 30), "it paired")
        app.buttons["Files"].tap()
        XCTAssertTrue(app.cells.firstMatch.waitForExistence(timeout: 10), "the drive is listed")
        keep("2 files")
        app.buttons["Sessions"].tap()

        // Opens a session that is running, or starts one.
        let session = app.cells.firstMatch
        if session.waitForExistence(timeout: 5) {
            keep("2 sessions")
            session.tap()
        } else {
            app.buttons["New Terminal"].tap()
        }

        let terminal = app.otherElements["Terminal"]
        XCTAssertTrue(terminal.waitForExistence(timeout: 10))
        sleep(3)
        keep("3 session")
        terminal.tap()
        app.typeText("echo typed on the phone\n")
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
