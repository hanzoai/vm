use std::os::fd::RawFd;
use std::sync::atomic::{AtomicBool, Ordering};

/// Saved terminal state for later restoration.
pub struct TerminalState {
    fd: RawFd,
    termios: libc::termios,
}

impl TerminalState {
    /// Save the current terminal attributes and switch to raw mode.
    /// Returns `None` if the fd is not a terminal.
    pub fn enter_raw_mode(fd: RawFd) -> Option<Self> {
        unsafe {
            let mut saved: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut saved) != 0 {
                return None;
            }
            let mut raw = saved;
            libc::cfmakeraw(&mut raw);
            libc::tcsetattr(fd, libc::TCSANOW, &raw);
            Some(TerminalState { fd, termios: saved })
        }
    }

    /// Restore the saved terminal attributes.
    pub fn restore(&self) {
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSANOW, &self.termios);
        }
    }
}

impl Drop for TerminalState {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Get the terminal size (rows, cols) for the given fd.
/// Returns (24, 80) as fallback if the ioctl fails.
pub fn terminal_size(fd: RawFd) -> (u16, u16) {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) == 0 {
            (ws.ws_row, ws.ws_col)
        } else {
            (24, 80)
        }
    }
}

/// Poll a file descriptor for readability.
/// Returns `true` if the fd has data available, `false` on timeout or error.
pub fn poll_read(fd: RawFd, timeout_ms: i32) -> bool {
    unsafe {
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ret = libc::poll(&mut pfd, 1, timeout_ms);
        ret > 0 && pfd.revents & libc::POLLIN != 0
    }
}

/// Read bytes from a raw file descriptor.
/// Returns the number of bytes read, or 0 on EOF/error.
pub fn read_raw(fd: RawFd, buf: &mut [u8]) -> usize {
    unsafe {
        let n = libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len());
        if n > 0 { n as usize } else { 0 }
    }
}

// --- SIGWINCH handling ---

static SIGWINCH_RECEIVED: AtomicBool = AtomicBool::new(false);

extern "C" fn sigwinch_handler(_: libc::c_int) {
    SIGWINCH_RECEIVED.store(true, Ordering::SeqCst);
}

/// Install a SIGWINCH handler that sets an internal flag.
pub fn install_sigwinch_handler() {
    SIGWINCH_RECEIVED.store(false, Ordering::SeqCst);
    unsafe {
        libc::signal(
            libc::SIGWINCH,
            sigwinch_handler as *const () as libc::sighandler_t,
        );
    }
}

/// Check if SIGWINCH was received since the last check. Clears the flag.
pub fn sigwinch_received() -> bool {
    SIGWINCH_RECEIVED.swap(false, Ordering::SeqCst)
}

/// Reset SIGWINCH handling to the system default.
pub fn reset_sigwinch_handler() {
    unsafe {
        libc::signal(libc::SIGWINCH, libc::SIG_DFL);
    }
}
