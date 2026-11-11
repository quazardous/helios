//! The system asks Helios to end: it closes as Ctrl+Q closes it.
//!
//! A plain `kill`, a logout or a shutdown sends SIGTERM; Ctrl+C in the
//! terminal Helios started from sends SIGINT, closing that terminal
//! SIGHUP. On Windows, the console Helios attached to closes or is
//! interrupted (`SetConsoleCtrlHandler`), and the session ends
//! (`WM_ENDSESSION`, which winit does not turn into a close request).
//! Left to the system's default, each ended the process on the spot: the
//! crash marker (`egui_pop::platform::crash`) stayed, and the next start
//! said Helios had closed unexpectedly.
//!
//! [`Ending`] takes these requests: the first asks the window to close
//! (the same close request Ctrl+Q and the window manager send, run on
//! the interface's thread), and the quit goes as usual, the crash marker
//! taken away by the `app.exit` step. A quit not done by
//! [`DEADLINE`], a second request, or a request before the window is
//! open, end the process at once, the session marked clean first
//! ([`mark_clean`]): the system asked, nothing crashed.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long the quit a request started may take before the process
/// ends anyway: the window closing and the vault's own deadline
/// (`quit::VAULT_DEADLINE`) fit well within it.
pub const DEADLINE: Duration = Duration::from_secs(8);

/// What closes the window: true when it was asked, false when there is
/// no window yet.
pub type Close = Box<dyn Fn() -> bool + Send + Sync>;
/// What ends the process at once, with an exit code.
pub type Exit = Box<dyn Fn(i32) + Send + Sync>;

/// The requests to end, and what they do.
pub struct Ending {
    close: Close,
    exit: Exit,
    deadline: Duration,
    asked: AtomicBool,
    finished: AtomicBool,
}

impl std::fmt::Debug for Ending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ending")
            .field("deadline", &self.deadline)
            .field("asked", &self.asked)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl Ending {
    pub fn new(close: Close, exit: Exit, deadline: Duration) -> Arc<Ending> {
        Arc::new(Ending {
            close,
            exit,
            deadline,
            asked: AtomicBool::new(false),
            finished: AtomicBool::new(false),
        })
    }

    /// The system asks Helios to end (`why`, for the log); `code` is the
    /// exit code should the process end at once.
    pub fn request(self: &Arc<Self>, why: &str, code: i32) {
        if self.finished() {
            return;
        }
        if self.asked.swap(true, Ordering::SeqCst) {
            log::warn!(target: "helios::quit", "{why} again: ending now");
            (self.exit)(code);
            return;
        }
        if !(self.close)() {
            log::info!(target: "helios::quit", "{why} before the window opened: ending now");
            (self.exit)(code);
            return;
        }
        log::info!(target: "helios::quit", "{why}: closing");
        let me = self.clone();
        let watch = std::thread::Builder::new()
            .name("helios-ending".into())
            .spawn(move || {
                if !me.wait(me.deadline) {
                    log::warn!(
                        target: "helios::quit",
                        "{} s after the request the quit is not done: ending now",
                        me.deadline.as_secs()
                    );
                    (me.exit)(code);
                }
            });
        if let Err(e) = watch {
            log::warn!(target: "helios::quit", "no thread to watch the quit's deadline: {e}");
        }
    }

    /// The quit is done: the deadline no longer applies.
    pub fn finish(&self) {
        self.finished.store(true, Ordering::SeqCst);
    }

    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    /// Waits up to `limit` for the quit to be done: whether it is.
    pub fn wait(&self, limit: Duration) -> bool {
        let until = Instant::now() + limit;
        while !self.finished() {
            let now = Instant::now();
            if now >= until {
                return false;
            }
            std::thread::sleep((until - now).min(Duration::from_millis(20)));
        }
        true
    }
}

/// Marks this session as ended cleanly in the crash directory `dir`: its
/// marker and its panics go, as the `app.exit` step takes them (the files
/// `egui_pop::platform::crash` writes, by this process's id).
pub fn mark_clean(dir: &Path) {
    let pid = std::process::id();
    for name in [format!("session-{pid}.json"), format!("panic-{pid}.json")] {
        let _ = std::fs::remove_file(dir.join(name));
    }
}

/// Unix: SIGTERM, SIGINT and SIGHUP become requests to `ending`, read on
/// a thread of their own (their handlers only wake it); the exit code
/// of an immediate end is the shell's for the signal (128 + its number).
#[cfg(unix)]
pub fn listen(ending: Arc<Ending>) -> std::io::Result<()> {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    listen_to(ending, &[SIGTERM, SIGINT, SIGHUP])
}

/// [`listen`] to the signals named.
#[cfg(unix)]
pub fn listen_to(ending: Arc<Ending>, signals: &[i32]) -> std::io::Result<()> {
    let mut signals = signal_hook::iterator::Signals::new(signals)?;
    std::thread::Builder::new()
        .name("helios-signals".into())
        .spawn(move || {
            for signal in signals.forever() {
                let name = signal_hook::low_level::signal_name(signal).unwrap_or("a signal");
                ending.request(name, 128 + signal);
            }
        })?;
    Ok(())
}

/// Windows: the console's events and the session's end become requests
/// to `ending`.
#[cfg(windows)]
pub fn listen(ending: Arc<Ending>) -> std::io::Result<()> {
    windows::listen(ending)
}

#[cfg(windows)]
#[allow(unsafe_code)] // Win32: a console handler and a hidden window
mod windows {
    use std::sync::{Arc, OnceLock};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
        SetConsoleCtrlHandler,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, RegisterClassW,
        WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
    };
    use windows_sys::core::BOOL;

    use super::Ending;

    /// The system ends the process once a console close handler returns
    /// (about 5 s after the event at most): the quit has a little less.
    const CONSOLE_WAIT: Duration = Duration::from_millis(4500);

    static ENDING: OnceLock<Arc<Ending>> = OnceLock::new();

    /// Waits for the quit a request started, then marks the session
    /// clean if it is not done: the system ends the process next.
    fn request_and_wait(why: &str, wait: Duration) {
        if let Some(ending) = ENDING.get() {
            ending.request(why, 0);
            if !ending.wait(wait) {
                (ending.exit)(0);
            }
        }
    }

    unsafe extern "system" fn on_console(event: u32) -> BOOL {
        match event {
            CTRL_C_EVENT | CTRL_BREAK_EVENT => {
                if let Some(ending) = ENDING.get() {
                    ending.request("Ctrl+C", 0);
                }
                1
            }
            CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
                request_and_wait("the console closing", CONSOLE_WAIT);
                1
            }
            _ => 0,
        }
    }

    unsafe extern "system" fn on_message(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            // Helios does not hold the session up.
            WM_QUERYENDSESSION => 1,
            WM_ENDSESSION if wparam != 0 => {
                request_and_wait("the session ending", super::DEADLINE);
                0
            }
            // SAFETY: the arguments are the ones the system passed.
            _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
    }

    pub fn listen(ending: Arc<Ending>) -> std::io::Result<()> {
        let _ = ENDING.set(ending);
        // SAFETY: the handler is a plain function living as long as the
        // process.
        if unsafe { SetConsoleCtrlHandler(Some(on_console), 1) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // The session's end goes to top-level windows: a hidden one, on
        // its own thread, hears it (a message-only window would not).
        std::thread::Builder::new()
            .name("helios-session-end".into())
            .spawn(|| {
                let class: Vec<u16> = "HeliosSessionEnd\0".encode_utf16().collect();
                // SAFETY: the class and the window name point at
                // NUL-terminated strings that outlive the window (this
                // thread's loop); the window procedure is a plain function.
                unsafe {
                    let instance = GetModuleHandleW(std::ptr::null());
                    let wc = WNDCLASSW {
                        lpfnWndProc: Some(on_message),
                        hInstance: instance,
                        lpszClassName: class.as_ptr(),
                        ..WNDCLASSW::default()
                    };
                    if RegisterClassW(&wc) == 0 {
                        log::warn!(target: "helios::quit", "the session's end is not heard: no window class");
                        return;
                    }
                    let window = CreateWindowExW(
                        WS_EX_TOOLWINDOW,
                        class.as_ptr(),
                        class.as_ptr(),
                        WS_OVERLAPPED,
                        0,
                        0,
                        0,
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        instance,
                        std::ptr::null(),
                    );
                    if window.is_null() {
                        log::warn!(target: "helios::quit", "the session's end is not heard: no window");
                        return;
                    }
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                        DispatchMessageW(&msg);
                    }
                }
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc;

    /// An ending whose close and exit say so on channels.
    fn ending(
        open: bool,
        deadline: Duration,
    ) -> (Arc<Ending>, mpsc::Receiver<()>, mpsc::Receiver<i32>) {
        let (closed, closes) = mpsc::channel();
        let (exited, exits) = mpsc::channel();
        let closed = Mutex::new(closed);
        let exited = Mutex::new(exited);
        let e = Ending::new(
            Box::new(move || {
                let _ = closed.lock().unwrap().send(());
                open
            }),
            Box::new(move |code| {
                let _ = exited.lock().unwrap().send(code);
            }),
            deadline,
        );
        (e, closes, exits)
    }

    const LONG: Duration = Duration::from_secs(30);
    const SHORT: Duration = Duration::from_millis(50);

    #[test]
    fn the_first_request_closes_the_window_a_second_ends_now() {
        let (e, closes, exits) = ending(true, LONG);
        e.request("SIGTERM", 143);
        closes.recv_timeout(SHORT).unwrap();
        assert!(exits.recv_timeout(SHORT).is_err());
        e.request("SIGTERM", 143);
        assert_eq!(exits.recv_timeout(SHORT), Ok(143));
        assert!(closes.try_recv().is_err());
    }

    #[test]
    fn a_quit_past_its_deadline_ends_now() {
        let (e, closes, exits) = ending(true, SHORT);
        e.request("SIGTERM", 143);
        closes.recv_timeout(SHORT).unwrap();
        assert_eq!(exits.recv_timeout(Duration::from_secs(10)), Ok(143));
    }

    #[test]
    fn a_quit_done_in_time_is_left_to_end() {
        let (e, _closes, exits) = ending(true, Duration::from_millis(300));
        e.request("SIGTERM", 143);
        e.finish();
        assert!(exits.recv_timeout(Duration::from_secs(1)).is_err());
        // A request after the quit changes nothing.
        e.request("SIGTERM", 143);
        assert!(exits.recv_timeout(SHORT).is_err());
    }

    #[test]
    fn a_request_before_the_window_ends_now() {
        let (e, closes, exits) = ending(false, LONG);
        e.request("SIGINT", 130);
        closes.recv_timeout(SHORT).unwrap();
        assert_eq!(exits.recv_timeout(SHORT), Ok(130));
    }

    #[test]
    fn a_clean_mark_leaves_no_report() {
        use egui_pop::platform::crash::{self, Report};
        let dir = std::env::temp_dir().join(format!("helios-ending-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // This process's session, with a panic a fault boundary caught.
        crash::simulate(
            &dir,
            std::process::id(),
            &Report::panic("Helios", "0", "caught"),
        )
        .unwrap();
        mark_clean(&dir);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        assert!(crash::collect(&dir, None).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_signal_becomes_a_request() {
        use signal_hook::consts::SIGUSR2;
        let (e, closes, exits) = ending(true, LONG);
        listen_to(e, &[SIGUSR2]).unwrap();
        signal_hook::low_level::raise(SIGUSR2).unwrap();
        closes.recv_timeout(Duration::from_secs(5)).unwrap();
        signal_hook::low_level::raise(SIGUSR2).unwrap();
        assert_eq!(
            exits.recv_timeout(Duration::from_secs(5)),
            Ok(128 + SIGUSR2)
        );
    }
}
