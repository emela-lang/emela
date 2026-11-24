//! Progress on standard error while a command waits on the network or another program.
//!
//! It is shown only when standard error is a terminal and the command does not run inside a
//! coding agent; otherwise nothing is written, so what pipes and tests see is unchanged. It is
//! not part of the specification.
//!
//! A [`StatusLine`] has one line that is redrawn in place (a spinner and a message) and lines
//! that are kept above it ([`StatusLine::keep`]). [`StatusLine::stop`] erases the redrawn line,
//! so the output that follows starts at the beginning of a line; dropping the value stops it too.
//! Nothing else may write to the terminal between creating a status line and stopping it.

use std::io::{IsTerminal, Write};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const TICK: Duration = Duration::from_millis(80);
/// The width when `COLUMNS` does not give one. The standard library cannot ask the terminal.
const DEFAULT_WIDTH: usize = 80;
/// Moves to the start of the line and erases it.
const ERASE: &str = "\r\x1b[K";

pub struct StatusLine {
    /// `None` when progress is not shown.
    shared: Option<Arc<Mutex<State>>>,
    ticker: Mutex<Option<JoinHandle<()>>>,
}

struct State {
    out: Box<dyn Write + Send>,
    width: usize,
    message: Option<String>,
    frame: usize,
    drawn: bool,
    stopped: bool,
}

impl StatusLine {
    /// On standard error when it is a terminal and no coding agent runs the command; otherwise a
    /// status line that writes nothing.
    pub fn stderr() -> StatusLine {
        let shown = std::io::stderr().is_terminal()
            && !emela_driver::runs_in_agent(|name| std::env::var_os(name));
        if !shown {
            return StatusLine::hidden();
        }
        let width = std::env::var("COLUMNS")
            .ok()
            .and_then(|c| c.parse().ok())
            .filter(|&w| w > 0)
            .unwrap_or(DEFAULT_WIDTH);
        StatusLine::to(Box::new(std::io::stderr()), width, true)
    }

    pub fn hidden() -> StatusLine {
        StatusLine {
            shared: None,
            ticker: Mutex::new(None),
        }
    }

    /// Writes to `out`, cutting the redrawn line to `width` columns. With `tick`, a thread turns
    /// the spinner while the message stays the same.
    fn to(out: Box<dyn Write + Send>, width: usize, tick: bool) -> StatusLine {
        let shared = Arc::new(Mutex::new(State {
            out,
            width,
            message: None,
            frame: 0,
            drawn: false,
            stopped: false,
        }));
        let ticker = tick.then(|| {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                loop {
                    std::thread::park_timeout(TICK);
                    let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                    if state.stopped {
                        break;
                    }
                    if state.message.is_some() {
                        state.frame = (state.frame + 1) % FRAMES.len();
                        state.draw();
                    }
                }
            })
        });
        StatusLine {
            shared: Some(shared),
            ticker: Mutex::new(ticker),
        }
    }

    /// Shows `message` on the redrawn line.
    pub fn set(&self, message: String) {
        self.with(|state| {
            state.message = Some(message);
            state.draw();
        });
    }

    /// Writes `line`, where it stays, in place of the redrawn line. The work the redrawn line
    /// showed is over, so it is shown again only by the next [`StatusLine::set`].
    pub fn keep(&self, line: &str) {
        self.with(|state| {
            state.erase();
            state.message = None;
            let _ = writeln!(state.out, "{line}");
        });
    }

    /// Erases the redrawn line and shows nothing more. The kept lines stay.
    pub fn stop(&self) {
        self.with(|state| {
            state.erase();
            state.message = None;
            state.stopped = true;
        });
        let ticker = self.ticker.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(ticker) = ticker {
            ticker.thread().unpark();
            let _ = ticker.join();
        }
    }

    fn with(&self, f: impl FnOnce(&mut State)) {
        if let Some(shared) = &self.shared {
            let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
            if !state.stopped {
                f(&mut state);
                let _ = state.out.flush();
            }
        }
    }
}

impl Drop for StatusLine {
    fn drop(&mut self) {
        self.stop();
    }
}

impl State {
    fn draw(&mut self) {
        let Some(message) = &self.message else {
            return;
        };
        // One column is left free so that the cursor never wraps to the next line, where `\r`
        // could not reach the line to erase.
        let text: String = format!("{} {message}", FRAMES[self.frame])
            .chars()
            .take(self.width.saturating_sub(1))
            .collect();
        let _ = write!(self.out, "{ERASE}{text}");
        self.drawn = true;
    }

    fn erase(&mut self) {
        if self.drawn {
            let _ = write!(self.out, "{ERASE}");
            self.drawn = false;
        }
    }
}

/// An elapsed time as kept lines show it: seconds with one decimal.
pub fn seconds(elapsed: Duration) -> String {
    format!("{:.1}s", elapsed.as_secs_f64())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A writer whose bytes the test reads afterwards.
    #[derive(Clone, Default)]
    pub(crate) struct Buffer(pub Arc<Mutex<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Buffer {
        pub(crate) fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    pub(crate) fn line(width: usize) -> (StatusLine, Buffer) {
        let buffer = Buffer::default();
        (
            StatusLine::to(Box::new(buffer.clone()), width, false),
            buffer,
        )
    }

    #[test]
    fn kept_lines_stay_and_the_status_is_erased() {
        let (status, out) = line(80);
        status.set("resolving a v1.0.0".into());
        status.keep("downloaded a v1.0.0");
        status.set("resolving b v2.0.0".into());
        status.stop();
        assert_eq!(
            out.text(),
            "\r\x1b[K⠋ resolving a v1.0.0\
             \r\x1b[Kdownloaded a v1.0.0\n\
             \r\x1b[K⠋ resolving b v2.0.0\
             \r\x1b[K"
        );
    }

    #[test]
    fn a_kept_line_without_a_status_is_only_the_line() {
        let (status, out) = line(80);
        status.keep("resolved 0 Pomes (0.0s)");
        drop(status);
        assert_eq!(out.text(), "resolved 0 Pomes (0.0s)\n");
    }

    #[test]
    fn the_status_is_cut_to_the_width() {
        let (status, out) = line(8);
        status.set("downloading".into());
        status.stop();
        assert_eq!(out.text(), "\r\x1b[K⠋ downl\r\x1b[K");
    }

    #[test]
    fn nothing_is_written_after_stop() {
        let (status, out) = line(80);
        status.stop();
        status.set("late".into());
        status.keep("late");
        assert_eq!(out.text(), "");
    }

    #[test]
    fn the_ticker_turns_the_spinner_and_ends_with_stop() {
        let buffer = Buffer::default();
        let status = StatusLine::to(Box::new(buffer.clone()), 80, true);
        status.set("waiting".into());
        std::thread::sleep(TICK * 3);
        status.stop();
        let text = buffer.text();
        assert!(text.contains("⠙ waiting"), "{text:?}");
        assert!(text.ends_with(ERASE), "{text:?}");
    }

    #[test]
    fn seconds_have_one_decimal() {
        assert_eq!(seconds(Duration::from_millis(1234)), "1.2s");
        assert_eq!(seconds(Duration::ZERO), "0.0s");
    }
}
