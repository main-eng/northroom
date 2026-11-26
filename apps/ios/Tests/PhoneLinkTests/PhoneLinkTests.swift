import Foundation
import Testing

@testable import PhoneLink

// Run with `swift test`, which needs Xcode.
struct PhoneLinkTests {
    let key = String(repeating: "ab", count: 32)

    func text(_ ask: Ask) -> String {
        String(decoding: ask.encoded(), as: UTF8.self)
    }

    @Test func aPairingLinkIsReadWithItsEscapes() throws {
        let link =
            "northroom://pair?key=\(key)&code=c0de&name=Tanner%27s%20Mac"
            + "&relay=https%3A%2F%2Frelay.example.com%2F&at=192.168.1.65%3A4433"
            + "&at=%5B2600%3A%3A1%5D%3A4433&colour=blue"
        let invite = try #require(Invite(link: link))
        #expect(invite.key == key)
        #expect(invite.code == "c0de")
        #expect(invite.host == "Tanner's Mac")
        #expect(invite.relay == "https://relay.example.com/")
        #expect(invite.direct == ["192.168.1.65:4433", "[2600::1]:4433"])
    }

    @Test func whatIsNotAPairingLinkIsRefused() {
        #expect(Invite(link: "https://example.com/?key=\(key)&code=c0de") == nil)
        #expect(Invite(link: "northroom://pair?code=c0de") == nil)
        #expect(Invite(link: "northroom://pair?key=\(key)") == nil)
        #expect(Invite(link: "northroom://pair?key=short&code=c0de") == nil)
    }

    @Test func asksAreTheJSONTheComputerReads() {
        #expect(text(.hello) == #"{"type":"hello"}"#)
        #expect(
            text(.watch(session: "s1", columns: 48, lines: 30))
                == #"{"columns":48,"lines":30,"session":"s1","type":"watch"}"#)
        #expect(
            text(.press(session: "s1", key: .interrupt))
                == #"{"key":"interrupt","session":"s1","type":"press"}"#)
        #expect(
            text(.scroll(session: "s1", lines: -3))
                == #"{"lines":-3,"session":"s1","type":"scroll"}"#)
        #expect(
            text(.pair(code: "c0de", name: "My phone"))
                == #"{"code":"c0de","name":"My phone","type":"pair"}"#)
    }

    @Test func theSessionsTheComputerListsAreRead() throws {
        let message = ##"""
            {"type":"sessions","sessions":[{"id":"s1","name":"Build","folder":"Work","doing":"",
            "agent":"Claude Code","activity":"needs-you"}]}
            """##
        guard case .sessions(let listed) = Tell(Data(message.utf8)) else {
            Issue.record("not read as sessions")
            return
        }
        #expect(listed.first?.activity == .needsYou)
        #expect(listed.first?.agent == "Claude Code")
    }

    @Test func aScreenIsReadWithWhatItLeavesOut() throws {
        let message = ##"""
            {"type":"screen","session":"s1","screen":{"columns":48,"lines":30,"ink":"#eeeeee",
            "ground":"#111111","cursor":[3,7],"rows":[[{"column":0,"text":"ok","bold":true}],[]]}}
            """##
        guard case .screen("s1", let shown) = Tell(Data(message.utf8)) else {
            Issue.record("not read as a screen")
            return
        }
        #expect(shown.rows[0][0].text == "ok")
        #expect(shown.rows[0][0].bold == true)
        #expect(shown.rows[0][0].ink == nil)
        #expect(shown.rows[1].isEmpty)
        #expect(shown.cursor == [3, 7])
    }

    @Test func aRefusalIsReadAndANewerMessageIsSkipped() {
        let refused = Tell(Data(#"{"type":"refused","why":"wrong-code"}"#.utf8))
        #expect(refused == .refused(why: "wrong-code"))
        #expect(Tell(Data(#"{"type":"something-newer"}"#.utf8)) == nil)
        #expect(Tell(Data("not json".utf8)) == nil)
    }
}
