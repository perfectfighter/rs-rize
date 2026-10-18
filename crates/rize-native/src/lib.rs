//! Windows foreground sampling and current-user credential protection.
//!
//! No executable paths, icons, URLs or native errors cross this module's boundary.
use chrono::{SecondsFormat, Utc};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

mod events;
pub use events::{SessionEvent, SessionWatcher};

pub const SAMPLE_INTERVAL_MS: u64 = 5_000;
const OFF: &str = "Activity tracking is off.";
const UNSUPPORTED: &str = "Automatic activity tracking is available on Windows.";
const UNAVAILABLE: &str = "Activity collection is unavailable. Toggle tracking to retry.";

pub fn clean_text(value: &str, limit: usize) -> String {
    static URL: OnceLock<Regex> = OnceLock::new();
    let cleaned: String = value
        .chars()
        .filter(|c| !('\u{0}'..='\u{1f}').contains(c) && !('\u{7f}'..='\u{9f}').contains(c))
        .collect();
    let cleaned = URL
        .get_or_init(|| {
            Regex::new(r"(?i)(?-u:\b)(?:https?|ftp|file)://\S+|(?-u:\b)www\.[^\s]+").unwrap()
        })
        .replace_all(&cleaned, "[address]");
    let mut units = 0;
    cleaned
        .trim()
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= limit
        })
        .collect()
}
fn title(value: &str) -> String {
    static PRIVATE: OnceLock<Regex> = OnceLock::new();
    let clean = clean_text(value, 1024);
    if PRIVATE
        .get_or_init(|| {
            Regex::new(
                r"(?i)(?-u:\b)(?:incognito|inprivate|private\s+(?:browsing|window|mode))(?-u:\b)",
            )
            .unwrap()
        })
        .is_match(&clean)
    {
        "[Private window]".into()
    } else {
        clean
    }
}
fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
fn unavailable() -> Value {
    json!({"timestamp":timestamp(),"app":"","title":"","pid":0,"idleSeconds":0,"available":false})
}
fn allowed_sample(
    app: &str,
    raw_title: &str,
    pid: u32,
    idle_seconds: f64,
) -> Result<Value, String> {
    let app = clean_text(app, 256);
    if app.is_empty() || !idle_seconds.is_finite() || idle_seconds < 0. {
        return Err(UNAVAILABLE.into());
    }
    Ok(
        json!({"timestamp":timestamp(),"app":app,"title":title(raw_title),"pid":pid,"idleSeconds":idle_seconds,"available":true}),
    )
}
fn application_name(path: &str) -> String {
    // The full path exists only inside the native call and is never returned.
    let base = path.rsplit(['\\', '/']).next().unwrap_or("");
    let name = if base.to_ascii_lowercase().ends_with(".exe") {
        &base[..base.len() - 4]
    } else {
        base
    };
    clean_text(name, 256)
}

/// Synchronous sampler; the shell schedules it at SAMPLE_INTERVAL_MS.
/// Creation never samples, and stop/suspend prevent later scheduled calls collecting data.
pub struct Sampler {
    running: bool,
    available: bool,
    suspended: bool,
    message: String,
}
impl Default for Sampler {
    fn default() -> Self {
        Self {
            running: false,
            available: false,
            suspended: false,
            message: OFF.into(),
        }
    }
}
impl Sampler {
    pub fn start(&mut self) -> Value {
        if !cfg!(windows) {
            self.running = false;
            self.available = false;
            self.message = UNSUPPORTED.into();
            return unavailable();
        }
        self.running = true;
        self.sample()
    }
    pub fn stop(&mut self) {
        self.running = false;
        self.available = false;
        self.message = OFF.into();
    }
    pub fn set_suspended(&mut self, suspended: bool) {
        self.suspended = suspended;
        self.available = false;
        self.message = if suspended {
            "Activity tracking is suspended while the workstation is locked or asleep."
        } else if self.running {
            "Waiting for the next foreground sample."
        } else {
            OFF
        }
        .into();
    }
    pub fn sample(&mut self) -> Value {
        if !self.running || self.suspended {
            return unavailable();
        }
        match sample() {
            Ok(value) => {
                self.available = true;
                self.message = "Collecting local foreground activity.".into();
                value
            }
            Err(message) => {
                self.available = false;
                self.message = message;
                unavailable()
            }
        }
    }
    pub fn status(&self) -> Value {
        json!({"running":self.running,"available":self.available,"message":self.message})
    }
}

#[cfg(not(windows))]
pub fn sample() -> Result<Value, String> {
    Err(UNSUPPORTED.into())
}
#[cfg(windows)]
pub fn sample() -> Result<Value, String> {
    windows::sample_foreground()
}

pub fn secure_storage_available() -> bool {
    cfg!(windows)
}
pub fn protect_secret(secret: &str) -> Result<Vec<u8>, String> {
    encrypt_credentials(secret.as_bytes())
}
pub fn unprotect_secret(encrypted: &[u8]) -> Result<String, String> {
    String::from_utf8(decrypt_credentials(encrypted)?)
        .map_err(|_| "Stored credentials could not be decoded. Sign in again.".into())
}
#[cfg(not(windows))]
pub fn encrypt_credentials(_data: &[u8]) -> Result<Vec<u8>, String> {
    Err(
        "Secure credential storage is unavailable on this platform; sign-in stays in memory."
            .into(),
    )
}
#[cfg(not(windows))]
pub fn decrypt_credentials(_data: &[u8]) -> Result<Vec<u8>, String> {
    Err("Secure credential storage is unavailable on this platform; sign in again.".into())
}
#[cfg(windows)]
pub fn encrypt_credentials(data: &[u8]) -> Result<Vec<u8>, String> {
    windows::crypt(data, true)
}
#[cfg(windows)]
pub fn decrypt_credentials(data: &[u8]) -> Result<Vec<u8>, String> {
    windows::crypt(data, false)
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{mem::size_of, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, LocalFree, HANDLE},
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
        Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW},
        System::{
            StationsAndDesktops::{
                CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, DESKTOP_READOBJECTS,
                UOI_NAME,
            },
            SystemInformation::GetTickCount64,
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::{
            Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
            WindowsAndMessaging::{
                GetClassNameW, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
            },
        },
    };
    struct Process(HANDLE);
    impl Drop for Process {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }
    fn interactive_desktop() -> bool {
        // Access to the secure Winlogon desktop is deliberately not requested.
        // A locked workstation or inaccessible input desktop breaks continuity.
        unsafe {
            let desktop = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
            if desktop.is_null() {
                return false;
            }
            let mut buffer = [0u16; 256];
            let mut needed = 0u32;
            let ok = GetUserObjectInformationW(
                desktop as HANDLE,
                UOI_NAME,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 2) as u32,
                &mut needed,
            );
            CloseDesktop(desktop);
            if ok == 0 || needed as usize > buffer.len() * 2 {
                return false;
            }
            let length = buffer.iter().position(|v| *v == 0).unwrap_or(buffer.len());
            String::from_utf16_lossy(&buffer[..length]).eq_ignore_ascii_case("default")
        }
    }
    fn resource_application_name(path: &[u16]) -> Option<String> {
        // Retain friendly application names used by existing classification rules.
        // The executable path stays inside this adapter; only version text leaves it.
        unsafe {
            let mut ignored = 0;
            let size = GetFileVersionInfoSizeW(path.as_ptr(), &mut ignored);
            if size == 0 || size > 1_048_576 {
                return None;
            }
            let mut data = vec![0u32; (size as usize + 3) / 4];
            if GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
                return None;
            }
            let begin = data.as_ptr() as usize;
            let end = begin.checked_add(size as usize)?;
            let bounded = |pointer: *mut std::ffi::c_void, bytes: usize| {
                !pointer.is_null()
                    && (pointer as usize) >= begin
                    && (pointer as usize)
                        .checked_add(bytes)
                        .is_some_and(|last| last <= end)
            };
            let translation: Vec<u16> = "\\VarFileInfo\\Translation\0".encode_utf16().collect();
            let mut pointer = ptr::null_mut();
            let mut length = 0;
            if VerQueryValueW(
                data.as_ptr().cast(),
                translation.as_ptr(),
                &mut pointer,
                &mut length,
            ) == 0
                || length < 4
                || !bounded(pointer, length as usize)
            {
                return None;
            }
            let language = ptr::read_unaligned(pointer.cast::<u16>());
            let codepage = ptr::read_unaligned(pointer.cast::<u16>().add(1));
            for field in ["FileDescription", "ProductName"] {
                let key: Vec<u16> =
                    format!("\\StringFileInfo\\{language:04x}{codepage:04x}\\{field}\0")
                        .encode_utf16()
                        .collect();
                let mut pointer = ptr::null_mut();
                let mut length = 0;
                if VerQueryValueW(
                    data.as_ptr().cast(),
                    key.as_ptr(),
                    &mut pointer,
                    &mut length,
                ) == 0
                    || length == 0
                    || length > 2048
                    || (pointer as usize) % 2 != 0
                    || !bounded(pointer, length as usize * 2)
                {
                    continue;
                }
                let raw = std::slice::from_raw_parts(pointer.cast::<u16>(), length as usize);
                let end = raw.iter().position(|v| *v == 0).unwrap_or(raw.len());
                let value = clean_text(&String::from_utf16_lossy(&raw[..end]), 256);
                if !value.is_empty() && !value.contains(['\\', '/']) {
                    return Some(value);
                }
            }
            None
        }
    }
    pub fn sample_foreground() -> Result<Value, String> {
        if !interactive_desktop() {
            return Err(
                "Activity tracking is suspended while the workstation is locked or unavailable."
                    .into(),
            );
        }
        // All buffers are bounded, their API-reported lengths checked before decoding.
        unsafe {
            let mut idle = LASTINPUTINFO {
                cbSize: size_of::<LASTINPUTINFO>() as u32,
                dwTime: 0,
            };
            if GetLastInputInfo(&mut idle) == 0 {
                return Err(UNAVAILABLE.into());
            }
            let idle_seconds = (GetTickCount64() as u32).wrapping_sub(idle.dwTime) as f64 / 1000.;
            let hwnd = GetForegroundWindow();
            if hwnd.is_null() {
                return Err("No foreground application is available.".into());
            }
            let mut pid = 0u32;
            if GetWindowThreadProcessId(hwnd, &mut pid) == 0 || pid == 0 {
                return Err(UNAVAILABLE.into());
            }
            let process = Process(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid));
            if process.0.is_null() {
                return Err(UNAVAILABLE.into());
            }
            let mut path = vec![0u16; 32768];
            let mut path_length = path.len() as u32;
            if QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &mut path_length) == 0
                || path_length == 0
                || path_length as usize >= path.len()
            {
                return Err(UNAVAILABLE.into());
            }
            let app = resource_application_name(&path).unwrap_or_else(|| {
                application_name(&String::from_utf16_lossy(&path[..path_length as usize]))
            });
            let mut title_buffer = [0u16; 4097];
            let title_length =
                GetWindowTextW(hwnd, title_buffer.as_mut_ptr(), title_buffer.len() as i32);
            if title_length < 0 || title_length as usize >= title_buffer.len() {
                return Err(UNAVAILABLE.into());
            }
            let mut class_buffer = [0u16; 256];
            let class_length =
                GetClassNameW(hwnd, class_buffer.as_mut_ptr(), class_buffer.len() as i32);
            if class_length <= 0 || class_length as usize >= class_buffer.len() {
                return Err(UNAVAILABLE.into());
            }
            let class = String::from_utf16_lossy(&class_buffer[..class_length as usize]);
            if class.eq_ignore_ascii_case("Windows Default Lock Screen")
                || class.eq_ignore_ascii_case("LockScreen")
            {
                return Err("The workstation is locked.".into());
            }
            // A foreground switch while resolving metadata invalidates the point.
            if GetForegroundWindow() != hwnd || !interactive_desktop() {
                return Err("Foreground application changed; waiting for the next sample.".into());
            }
            allowed_sample(
                &app,
                &String::from_utf16_lossy(&title_buffer[..title_length as usize]),
                pid,
                idle_seconds,
            )
        }
    }
    pub fn crypt(data: &[u8], protect: bool) -> Result<Vec<u8>, String> {
        const ENTROPY: &[u8] = b"rs-rize/cloud-session/v1";
        if data.is_empty() || data.len() > 65_536 {
            return Err("Stored credentials have an invalid size. Sign in again.".into());
        }
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let entropy = CRYPT_INTEGER_BLOB {
            cbData: ENTROPY.len() as u32,
            pbData: ENTROPY.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };
        let description: Vec<u16> = "rs-rize session\0".encode_utf16().collect();
        unsafe {
            let ok = if protect {
                CryptProtectData(
                    &input,
                    description.as_ptr(),
                    &entropy,
                    ptr::null(),
                    ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            } else {
                CryptUnprotectData(
                    &input,
                    ptr::null_mut(),
                    &entropy,
                    ptr::null(),
                    ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            };
            if ok == 0 {
                return Err(
                    "Secure credential storage could not be accessed. Sign in again.".into(),
                );
            }
            if output.pbData.is_null() || output.cbData == 0 || output.cbData > 1_048_576 {
                if !output.pbData.is_null() {
                    LocalFree(output.pbData.cast());
                }
                return Err(
                    "Secure credential storage returned an invalid result. Sign in again.".into(),
                );
            }
            let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            // DPAPI owns this allocation. Clear its decrypted copy before releasing it.
            for index in 0..output.cbData as usize {
                ptr::write_volatile(output.pbData.add(index), 0);
            }
            LocalFree(output.pbData.cast());
            Ok(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn creation_and_stop_never_collect() {
        let mut s = Sampler::default();
        assert_eq!(
            s.status(),
            json!({"running":false,"available":false,"message":OFF})
        );
        assert_eq!(s.sample()["available"], false);
        s.stop();
        assert_eq!(s.sample()["available"], false);
    }
    #[test]
    fn allowlist_redacts_private_titles_and_urls() {
        let a = allowed_sample(
            "Br\0owser\u{85}",
            "Secret https://secret.test - Incognito",
            42,
            3.,
        )
        .unwrap();
        assert_eq!(a["app"], "Browser");
        assert_eq!(a["title"], "[Private window]");
        assert_eq!(a.as_object().unwrap().len(), 6);
        for key in ["path", "icon", "url", "class"] {
            assert!(a.get(key).is_none());
        }
        assert_eq!(
            allowed_sample("Editor", "A\r\nB\tC\u{1b}", 0, 0.).unwrap()["title"],
            "ABC"
        );
        assert_eq!(
            title("Visit https://secret.test/account and www.private.test today"),
            "Visit [address] and [address] today"
        );
    }
    #[test]
    fn data_bounds_and_invalid_idle_are_explicit() {
        assert!(allowed_sample("", "Title", 4, 0.).is_err());
        assert!(allowed_sample("Editor", "Title", 4, f64::NAN).is_err());
        assert!(allowed_sample("Editor", "Title", 4, -1.).is_err());
        assert_eq!(clean_text(&"a".repeat(1000), 256).len(), 256);
        assert_eq!(clean_text("😀😀", 3), "😀");
    }
    #[test]
    fn executable_paths_never_escape_as_app_names() {
        assert_eq!(application_name(r"C:\Users\secret\Editor.exe"), "Editor");
        assert_eq!(application_name(r"C:\private\Code.EXE"), "Code");
        assert_eq!(application_name("/private/app.exe"), "app");
        assert_eq!(application_name("abéø你"), "abéø你");
    }
    #[test]
    fn suspended_sampler_cannot_credit_work() {
        let mut s = Sampler::default();
        s.set_suspended(true);
        assert_eq!(s.sample()["available"], false);
        assert_eq!(s.status()["available"], false);
        s.set_suspended(false);
        assert_eq!(s.status()["message"], OFF);
    }
    #[cfg(not(windows))]
    #[test]
    fn unsupported_platform_is_explicit_and_never_persists_plaintext() {
        let mut s = Sampler::default();
        assert_eq!(s.start()["available"], false);
        assert_eq!(s.status()["running"], false);
        assert!(!secure_storage_available());
        assert!(protect_secret("secret-token").is_err());
        assert!(unprotect_secret(b"not-encrypted").is_err());
    }
    #[cfg(windows)]
    #[test]
    fn dpapi_current_user_round_trip_rejects_tampering() {
        let encrypted = protect_secret("test-only-session-token").unwrap();
        assert_ne!(encrypted, b"test-only-session-token");
        assert_eq!(
            unprotect_secret(&encrypted).unwrap(),
            "test-only-session-token"
        );
        let mut changed = encrypted;
        let i = changed.len() / 2;
        changed[i] ^= 0x80;
        assert!(unprotect_secret(&changed).is_err());
        assert!(protect_secret("").is_err());
    }
}
