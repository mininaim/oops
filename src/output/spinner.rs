//! A delayed terminal spinner, fully isolated from diagnosis logic.
//!
//! The spinner starts a background thread that stays silent for a grace
//! period; if the work finishes first, nothing is ever drawn — no flicker
//! on the common fast path. Once visible, it animates on stderr and erases
//! itself completely before results are printed, so nothing is left in
//! scrollback. It never delays completion: finishing wakes the thread
//! immediately via a condvar.
//!
//! The frames are the oops mark in motion: a path corner turning through
//! `╭ ╮ ╯ ╰` — a route being tried in every direction until it resolves.

use std::io::Write;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const FRAMES: [&str; 4] = ["╭", "╮", "╯", "╰"];
const ASCII_FRAMES: [&str; 4] = ["-", "\\", "|", "/"];
const FRAME_INTERVAL: Duration = Duration::from_millis(120);
/// How long inspection may run before the spinner becomes visible.
const VISIBILITY_DELAY: Duration = Duration::from_millis(150);
/// After this long, the message acknowledges the wait without drama.
const SLOW_AFTER: Duration = Duration::from_secs(2);
const MESSAGE: &str = "inspecting repository…";
const SLOW_MESSAGE: &str = "still inspecting…";
/// Carriage return + erase-line, leaving the cursor at column 0.
pub const ERASE_LINE: &str = "\r\x1b[2K";

type StopSignal = Arc<(Mutex<bool>, Condvar)>;

pub struct Spinner {
    inner: Option<Inner>,
}

struct Inner {
    stop: StopSignal,
    handle: JoinHandle<()>,
}

impl Spinner {
    /// Starts the spinner on stderr, or an inert one when `enabled` is false
    /// (non-interactive terminals, NO_COLOR, --json).
    pub fn start(enabled: bool) -> Self {
        if !enabled {
            return Spinner { inner: None };
        }
        let frames = if super::locale_supports_unicode() {
            &FRAMES
        } else {
            &ASCII_FRAMES
        };
        Self::start_with(Box::new(std::io::stderr()), VISIBILITY_DELAY, frames)
    }

    /// Testable constructor: any writer, any visibility delay.
    pub fn start_with(
        writer: Box<dyn Write + Send>,
        delay: Duration,
        frames: &'static [&'static str; 4],
    ) -> Self {
        let stop: StopSignal = Arc::new((Mutex::new(false), Condvar::new()));
        let thread_stop = Arc::clone(&stop);
        let mut writer = writer;
        let handle = std::thread::spawn(move || animate(&mut *writer, &thread_stop, delay, frames));
        Spinner {
            inner: Some(Inner { stop, handle }),
        }
    }

    /// Stops the animation and erases any drawn frame. Also runs on Drop,
    /// so an early error return still cleans the line up.
    pub fn finish(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        if let Some(inner) = self.inner.take() {
            *inner.stop.0.lock().unwrap() = true;
            inner.stop.1.notify_all();
            let _ = inner.handle.join();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

fn animate(
    out: &mut dyn Write,
    stop: &(Mutex<bool>, Condvar),
    delay: Duration,
    frames: &[&str; 4],
) {
    let (lock, cvar) = stop;
    let mut stopped = lock.lock().unwrap();

    stopped = cvar.wait_timeout_while(stopped, delay, |s| !*s).unwrap().0;
    if *stopped {
        return; // finished within the grace period: draw nothing at all
    }

    let shown_since = Instant::now();
    let mut frame = 0usize;
    while !*stopped {
        let message = if shown_since.elapsed() >= SLOW_AFTER {
            SLOW_MESSAGE
        } else {
            MESSAGE
        };
        // The spinner only runs on ANSI-capable interactive terminals,
        // so a plain dim SGR here is always safe.
        let _ = write!(
            out,
            "{ERASE_LINE}{} \x1b[2m{message}\x1b[0m",
            frames[frame % frames.len()]
        );
        let _ = out.flush();
        frame += 1;
        stopped = cvar
            .wait_timeout_while(stopped, FRAME_INTERVAL, |s| !*s)
            .unwrap()
            .0;
    }
    let _ = write!(out, "{ERASE_LINE}");
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[derive(Clone, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl SharedBuf {
        fn contents(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn fast_completion_draws_nothing() {
        let buf = SharedBuf::default();
        let spinner =
            Spinner::start_with(Box::new(buf.clone()), Duration::from_millis(250), &FRAMES);
        thread::sleep(Duration::from_millis(5));
        spinner.finish();
        assert_eq!(buf.contents(), "", "no frames, no cleanup, no flicker");
    }

    #[test]
    fn slow_completion_draws_frames_then_erases_them() {
        let buf = SharedBuf::default();
        let spinner =
            Spinner::start_with(Box::new(buf.clone()), Duration::from_millis(10), &FRAMES);
        thread::sleep(Duration::from_millis(160));
        spinner.finish();
        let text = buf.contents();
        assert!(text.contains("inspecting repository"), "{text:?}");
        assert!(text.contains(FRAMES[0]), "{text:?}");
        assert!(
            text.ends_with(ERASE_LINE),
            "must leave a clean line behind: {text:?}"
        );
    }

    #[test]
    fn disabled_spinner_is_inert() {
        let spinner = Spinner::start(false);
        spinner.finish();
    }

    #[test]
    fn finish_does_not_wait_for_the_grace_period() {
        let buf = SharedBuf::default();
        let started = Instant::now();
        let spinner = Spinner::start_with(Box::new(buf.clone()), Duration::from_secs(5), &FRAMES);
        spinner.finish();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "finish must interrupt the delay immediately"
        );
        assert_eq!(buf.contents(), "");
    }
}
