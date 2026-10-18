//! Immediate session/power boundaries; polling alone cannot observe a short lock.
use std::sync::mpsc;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    Lock,
    Unlock,
    Suspend,
    Resume,
}

pub struct SessionWatcher {
    receiver: mpsc::Receiver<SessionEvent>,
    #[cfg(windows)]
    thread: Option<std::thread::JoinHandle<()>>,
    #[cfg(windows)]
    window: usize,
    #[cfg(windows)]
    stop_message: u32,
}
impl SessionWatcher {
    #[cfg(not(windows))]
    pub fn start() -> Result<Self, String> {
        Err("Windows session notifications are unavailable on this platform.".into())
    }
    #[cfg(windows)]
    pub fn start() -> Result<Self, String> {
        windows::start()
    }
    pub fn try_recv(&self) -> Option<SessionEvent> {
        self.receiver.try_recv().ok()
    }
    pub fn is_running(&self) -> bool {
        #[cfg(windows)]
        {
            self.thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
}
#[cfg(windows)]
impl Drop for SessionWatcher {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                self.window as _,
                self.stop_message,
                0,
                0,
            );
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn message_event(message: u32, value: usize) -> Option<SessionEvent> {
    match (message, value) {
        // WM_WTSSESSION_CHANGE: local/remote disconnect, lock; connect and unlock.
        (0x02b1, 2 | 4 | 7) => Some(SessionEvent::Lock),
        (0x02b1, 1 | 3 | 8) => Some(SessionEvent::Unlock),
        // WM_POWERBROADCAST: PBT_APMSUSPEND and all documented resume notifications.
        (0x0218, 4) => Some(SessionEvent::Suspend),
        (0x0218, 6 | 7 | 18) => Some(SessionEvent::Resume),
        _ => None,
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{
        ptr,
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::{
            LibraryLoader::GetModuleHandleW,
            RemoteDesktop::{WTSRegisterSessionNotification, WTSUnRegisterSessionNotification},
        },
        UI::WindowsAndMessaging::*,
    };
    struct Events {
        sender: mpsc::Sender<SessionEvent>,
        stop_message: u32,
    }
    unsafe extern "system" fn procedure(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_NCCREATE {
            let create = lparam as *const CREATESTRUCTW;
            if create.is_null() {
                return 0;
            }
            SetWindowLongPtrW(window, GWLP_USERDATA, (*create).lpCreateParams as isize);
            return 1;
        }
        let events = GetWindowLongPtrW(window, GWLP_USERDATA) as *const Events;
        if !events.is_null() && message == (*events).stop_message {
            WTSUnRegisterSessionNotification(window);
            DestroyWindow(window);
            return 0;
        }
        if let Some(event) = message_event(message, wparam) {
            if !events.is_null() {
                let _ = (*events).sender.send(event);
            }
            return 1;
        }
        match message {
            WM_CLOSE => {
                WTSUnRegisterSessionNotification(window);
                DestroyWindow(window);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(window, GWLP_USERDATA, 0);
                DefWindowProcW(window, message, wparam, lparam)
            }
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
    pub fn start() -> Result<SessionWatcher, String> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let (sender, receiver) = mpsc::channel();
        let (ready, result) = mpsc::sync_channel(1);
        let name = format!(
            "RsRizeSessionObserver-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let thread = std::thread::Builder::new()
            .name("rs-rize-session-observer".into())
            .spawn(move || unsafe {
                let instance = GetModuleHandleW(ptr::null());
                if instance.is_null() {
                    let _ = ready.send(Err(
                        "Windows session notifications could not be initialized.".to_string(),
                    ));
                    return;
                }
                let class: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
                let stop_message = RegisterWindowMessageW(class.as_ptr());
                if stop_message == 0 {
                    let _ = ready.send(Err(
                        "Windows session notifications could not be initialized.".to_string(),
                    ));
                    return;
                }
                let mut descriptor: WNDCLASSW = std::mem::zeroed();
                descriptor.lpfnWndProc = Some(procedure);
                descriptor.hInstance = instance;
                descriptor.lpszClassName = class.as_ptr();
                if RegisterClassW(&descriptor) == 0 {
                    let _ = ready.send(Err(
                        "Windows session notifications could not be registered.".to_string(),
                    ));
                    return;
                }
                // A hidden ordinary top-level window receives WM_POWERBROADCAST;
                // message-only windows intentionally do not receive that broadcast.
                let event_pointer = Box::into_raw(Box::new(Events {
                    sender,
                    stop_message,
                }));
                let window = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    class.as_ptr(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    instance,
                    event_pointer.cast(),
                );
                if window.is_null() {
                    drop(Box::from_raw(event_pointer));
                    UnregisterClassW(class.as_ptr(), instance);
                    let _ = ready.send(Err(
                        "The Windows session observer could not be opened.".to_string()
                    ));
                    return;
                }
                // NOTIFY_FOR_THIS_SESSION = 0. Never subscribe to another user's session.
                if WTSRegisterSessionNotification(window, 0) == 0 {
                    DestroyWindow(window);
                    drop(Box::from_raw(event_pointer));
                    UnregisterClassW(class.as_ptr(), instance);
                    let _ = ready.send(Err(
                        "Windows lock notifications are unavailable. Toggle tracking to retry."
                            .to_string(),
                    ));
                    return;
                }
                if ready.send(Ok((window as usize, stop_message))).is_err() {
                    WTSUnRegisterSessionNotification(window);
                    DestroyWindow(window);
                    drop(Box::from_raw(event_pointer));
                    UnregisterClassW(class.as_ptr(), instance);
                    return;
                }
                let mut message: MSG = std::mem::zeroed();
                while GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                WTSUnRegisterSessionNotification(window);
                // GetMessage may fail independently of a close. Ensure the native
                // callback can no longer access its sender before releasing that Box.
                if IsWindow(window) != 0 {
                    DestroyWindow(window);
                }
                drop(Box::from_raw(event_pointer));
                UnregisterClassW(class.as_ptr(), instance);
            })
            .map_err(|_| "The Windows session observer thread could not be started.".to_string())?;
        match result.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok((window,stop_message)))=>Ok(SessionWatcher {receiver,thread:Some(thread),window,stop_message}),
            Ok(Err(error))=>{let _=thread.join();Err(error)},
            // Closing the readiness channel makes a delayed observer tear itself
            // down. A stuck OS call cannot block application startup indefinitely.
            Err(_)=>Err("Windows session notifications took too long to initialize. Toggle tracking to retry.".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_session_boundaries_map_without_collecting_metadata() {
        for code in [2, 4, 7] {
            assert_eq!(message_event(0x2b1, code), Some(SessionEvent::Lock));
        }
        for code in [1, 3, 8] {
            assert_eq!(message_event(0x2b1, code), Some(SessionEvent::Unlock));
        }
        assert_eq!(message_event(0x218, 4), Some(SessionEvent::Suspend));
        for code in [6, 7, 18] {
            assert_eq!(message_event(0x218, code), Some(SessionEvent::Resume));
        }
        assert_eq!(message_event(0x218, 0), None);
        assert_eq!(message_event(0x2b1, 5), None);
        assert_eq!(message_event(0, 7), None);
    }
    #[cfg(not(windows))]
    #[test]
    fn session_watcher_is_explicitly_windows_only() {
        assert!(SessionWatcher::start().is_err());
    }
    #[cfg(windows)]
    #[test]
    fn session_observer_receives_boundaries_and_releases_its_thread() {
        use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;
        let watcher = SessionWatcher::start().expect("Windows session notifications available");
        assert!(watcher.is_running());
        for (message, value, event) in [
            (0x2b1, 7, SessionEvent::Lock),
            (0x2b1, 8, SessionEvent::Unlock),
            (0x218, 4, SessionEvent::Suspend),
            (0x218, 18, SessionEvent::Resume),
        ] {
            assert_ne!(
                unsafe { PostMessageW(watcher.window as _, message, value, 0) },
                0
            );
            assert_eq!(
                watcher
                    .receiver
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap(),
                event
            );
        }
        drop(watcher); // Drop joins the observer after unregistering and destroying its window.
    }
}
