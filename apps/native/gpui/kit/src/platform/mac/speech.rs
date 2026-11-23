//! Start Speaking and Stop Speaking (Edit ▸ Speech), as in AppKit text views.

use super::appkit::{Id, ns_string};
use objc2::{class, msg_send};

thread_local! {
    /// The app's single `NSSpeechSynthesizer`, using the user's chosen voice.
    static VOICE: Id = {
        // SAFETY: made once on the main thread and kept for the life of the app.
        unsafe {
            let voice: Id = msg_send![class!(NSSpeechSynthesizer), alloc];
            msg_send![voice, initWithVoice: std::ptr::null_mut::<objc2::runtime::AnyObject>()]
        }
    };
}

/// Reads `words` aloud, interrupting any current speech.
pub fn start_speaking(words: &str) {
    VOICE.with(|voice| {
        // SAFETY: the app's voice, on the main thread.
        unsafe {
            let _: bool = msg_send![*voice, startSpeakingString: ns_string(words)];
        }
    });
}

pub fn stop_speaking() {
    VOICE.with(|voice| {
        // SAFETY: the app's voice, on the main thread.
        unsafe {
            let _: () = msg_send![*voice, stopSpeaking];
        }
    });
}
