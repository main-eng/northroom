//! A pairing link as a QR code a phone's camera reads.

use qrcodegen::{QrCode, QrCodeEcc};

/// `text` as a QR code: each line's squares, true where dark, without the light edge around
/// it. `None` when the text is too long for a code.
pub fn modules(text: &str) -> Option<Vec<Vec<bool>>> {
    // The lowest error correction: a screen is clean, and coarser squares read from further.
    let code = QrCode::encode_text(text, QrCodeEcc::Low).ok()?;
    let side = code.size();
    Some(
        (0..side)
            .map(|line| {
                (0..side)
                    .map(|column| code.get_module(column, line))
                    .collect()
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_square_with_a_finder_in_its_corner() {
        let code = modules("northroom://pair?key=ab&code=cd").expect("a short link fits");
        assert!(code.iter().all(|row| row.len() == code.len()));
        // Every QR code starts with a 7 by 7 finder: a dark edge around a light ring.
        assert!(code[0][..7].iter().all(|dark| *dark));
        assert_eq!(
            code[1][..7],
            [true, false, false, false, false, false, true]
        );
    }

    #[test]
    fn text_longer_than_a_code_holds_is_refused() {
        assert_eq!(modules(&"x".repeat(8000)), None);
    }
}
