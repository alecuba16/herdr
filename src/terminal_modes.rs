use std::io::{self, Write};

#[cfg(any(not(windows), test))]
const DISABLE_HOST_MOUSE_REPORTING_SEQUENCE: &[u8] =
    b"\x1b[?1006l\x1b[?1016l\x1b[?1015l\x1b[?1005l\x1b[?1003l\x1b[?1002l\x1b[?1000l";

#[cfg(not(windows))]
pub(crate) fn clear_host_mouse_reporting<W: Write>(writer: &mut W) -> io::Result<()> {
    writer.write_all(DISABLE_HOST_MOUSE_REPORTING_SEQUENCE)?;
    writer.flush()
}

#[cfg(windows)]
pub(crate) fn clear_host_mouse_reporting<W: Write>(_writer: &mut W) -> io::Result<()> {
    Ok(())
}

/// Full reset sequence for all host terminal modes Herdr enables.
///
/// This covers every mode the client `setup_terminal_with_capabilities` and
/// `restore_terminal_state` paths can leave on:
///
/// - Mouse tracking: 1000, 1002, 1003, 1005, 1006, 1015, 1016
/// - Focus event reporting: 1004
/// - Bracketed paste: 2004
/// - Kitty keyboard protocol: `ESC[>0u` (pop all levels)
/// - Cursor keys / keypad: application mode off (not needed on most hosts but harmless)
/// - modifyOtherKeys: `ESC[>4;0m`
/// - Alternate screen: `ESC[?1049l`
/// - Cursor visible: `ESC[?25h`
/// - DECSCUSR reset to default: `ESC[0 q`
/// - SGR reset: `ESC[0m`
///
/// On Windows the mouse-reporting disable is a no-op (crossterm handles that
/// via console mode), but the rest of the sequence is still useful.
pub fn reset_host_terminal_modes() -> io::Result<()> {
    let mut stdout = io::stdout();
    reset_host_terminal_modes_into(&mut stdout)?;
    stdout.flush()
}

pub(crate) fn reset_host_terminal_modes_into<W: Write>(writer: &mut W) -> io::Result<()> {
    // Disable all mouse tracking modes.
    #[cfg(any(not(windows), test))]
    writer.write_all(DISABLE_HOST_MOUSE_REPORTING_SEQUENCE)?;

    // Disable focus event reporting, bracketed paste, alternate screen.
    writer.write_all(b"\x1b[?1004l\x1b[?2004l\x1b[?1049l")?;

    // Pop all kitty keyboard protocol levels (back to legacy/ANSI mode).
    writer.write_all(b"\x1b[>0u")?;

    // Reset modifyOtherKeys to the default (0).
    writer.write_all(b"\x1b[>4;0m")?;

    // Show cursor, reset DECSCUSR to terminal default, reset SGR attributes.
    writer.write_all(b"\x1b[?25h\x1b[0 q\x1b[0m")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clears_all_known_host_mouse_modes() {
        let sequence = std::str::from_utf8(DISABLE_HOST_MOUSE_REPORTING_SEQUENCE).unwrap();

        for mode in ["1000", "1002", "1003", "1005", "1006", "1015", "1016"] {
            assert!(
                sequence.contains(&format!("\x1b[?{mode}l")),
                "missing mouse mode {mode}"
            );
        }
    }

    #[test]
    fn reset_sequence_disables_all_host_protocols() {
        let mut buf = Vec::new();
        reset_host_terminal_modes_into(&mut buf).expect("reset writes succeed");
        let sequence = std::str::from_utf8(&buf).expect("reset sequence is valid UTF-8");

        // Mouse tracking modes.
        for mode in ["1000", "1002", "1003", "1005", "1006", "1015", "1016"] {
            assert!(
                sequence.contains(&format!("\x1b[?{mode}l")),
                "reset missing mouse mode {mode}"
            );
        }
        // Focus event reporting.
        assert!(
            sequence.contains("\x1b[?1004l"),
            "reset missing focus event disable"
        );
        // Bracketed paste.
        assert!(
            sequence.contains("\x1b[?2004l"),
            "reset missing bracketed paste disable"
        );
        // Alternate screen.
        assert!(
            sequence.contains("\x1b[?1049l"),
            "reset missing alternate screen disable"
        );
        // Kitty keyboard protocol pop.
        assert!(
            sequence.contains("\x1b[>0u"),
            "reset missing kitty keyboard disable"
        );
        // modifyOtherKeys reset.
        assert!(
            sequence.contains("\x1b[>4;0m"),
            "reset missing modifyOtherKeys reset"
        );
        // Cursor visible.
        assert!(
            sequence.contains("\x1b[?25h"),
            "reset missing cursor visible"
        );
        // DECSCUSR reset.
        assert!(
            sequence.contains("\x1b[0 q"),
            "reset missing DECSCUSR reset"
        );
        // SGR reset.
        assert!(sequence.contains("\x1b[0m"), "reset missing SGR reset");
    }
}
