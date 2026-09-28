use crossterm::event::{self, Event, KeyEventKind};
use std::io;
use std::time;

/// Read real terminal events.
pub fn read_event() -> io::Result<Option<Event>> {
    let event = event::read()?;

    // crossterm also emits key release and repeat events on Windows - skip other key events
    if let Event::Key(key_event) = event
        && key_event.kind != KeyEventKind::Press
    {
        return Ok(None);
    }

    Ok(Some(event))
}

pub fn read_event_with_timeout(timeout: time::Duration) -> io::Result<Option<Event>> {
    let event_available = event::poll(timeout)?;

    if event_available {
        read_event()
    } else {
        Ok(None)
    }
}

/// Reenable '\n' handling as in the default "cooked" terminal mode, even if raw mode is enabled.
///
/// Without this, '\n' doesn't result in carriage return.
#[cfg(unix)]
pub fn enable_output_postprocessing() -> io::Result<()> {
    use libc::TCSANOW;
    use std::os::unix::io::AsRawFd;
    use termios::*;

    let stdin_fd = io::stdin().as_raw_fd();
    let mut termios = Termios::from_fd(stdin_fd)?;
    termios.c_oflag |= libc::OPOST | libc::ONLCR;

    tcsetattr(stdin_fd, TCSANOW, &termios)
}
/// Do nothing since '\n' should be always reinterpreted as '\r\n' on Windows.
#[cfg(windows)]
pub fn enable_output_postprocessing() -> io::Result<()> {}
