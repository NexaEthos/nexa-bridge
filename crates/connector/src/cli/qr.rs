//! The approval link as a QR code a phone can scan from the terminal. Wiki: docs/wiki/server-setup.md

use anyhow::{Context, Result};
use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;

/// Render `text` as a QR code in half-block characters, light on dark like a terminal shows best.
pub fn render(text: &str) -> Result<String> {
    let code =
        QrCode::new(text.as_bytes()).context("cannot draw the approval link as a QR code")?;
    Ok(code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .quiet_zone(true)
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_approval_link_draws_as_a_square_of_half_blocks() -> Result<()> {
        let drawn = render("https://agora.example.com/bridge-pair#ABCD-EF12-3456")?;
        let rows: Vec<&str> = drawn.lines().collect();
        assert!(rows.len() > 10);
        assert!(
            rows.iter()
                .all(|row| row.chars().count() == rows[0].chars().count())
        );
        Ok(())
    }
}
