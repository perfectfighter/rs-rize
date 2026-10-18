//! Explicit, signed NSIS updates. The renderer cannot choose a feed, key, or executable.
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Update, UpdaterExt};

pub struct UpdateService {
    app: AppHandle,
    state: Arc<Mutex<Value>>,
    endpoint: Option<url::Url>,
    pubkey: String,
    pending: Option<Update>,
    downloaded: Option<Vec<u8>>,
}
fn support_error(
    windows: bool,
    debug: bool,
    nsis: bool,
    uninstaller: bool,
    key: &str,
    endpoint: &str,
    signed: bool,
) -> Option<String> {
    if !windows {
        return Some("Automatic installation is available in the Windows NSIS build.".into());
    }
    if debug || !nsis || !uninstaller {
        return Some("Install the NSIS build to use updates. Portable and development builds cannot update themselves.".into());
    }
    if key.trim().is_empty() || endpoint.trim().is_empty() {
        return Some(
            "This build has no published update configuration. Install a signed NSIS release."
                .into(),
        );
    }
    let valid = url::Url::parse(endpoint).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.fragment().is_none()
    });
    if !valid {
        return Some("The packaged update endpoint must use HTTPS without credentials.".into());
    }
    if !signed {
        return Some(
            "Signed updates require a configured publisher identity. Install a signed release."
                .into(),
        );
    }
    None
}
fn publish(app: &AppHandle, state: &Arc<Mutex<Value>>, patch: Value) -> Value {
    let mut current = state.lock().unwrap_or_else(|e| e.into_inner());
    for (k, v) in patch.as_object().unwrap() {
        current[k] = v.clone();
    }
    let snapshot = current.clone();
    drop(current);
    let _ = app.emit("rize:update-status", &snapshot);
    snapshot
}
impl UpdateService {
    pub fn new(app: AppHandle) -> Self {
        let key = option_env!("RIZE_UPDATER_PUBLIC_KEY")
            .unwrap_or("")
            .trim()
            .to_string();
        let endpoint = option_env!("RIZE_UPDATER_ENDPOINT").unwrap_or("").trim();
        let nsis =
            tauri::utils::platform::bundle_type() == Some(tauri::utils::config::BundleType::Nsis);
        let uninstaller = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("uninstall.exe")))
            .is_some_and(|p| p.is_file());
        let failure = support_error(
            cfg!(target_os = "windows"),
            cfg!(debug_assertions),
            nsis,
            uninstaller,
            &key,
            endpoint,
            option_env!("RIZE_AUTHENTICODE_SIGNED") == Some("1"),
        );
        let supported = failure.is_none();
        let message = failure.unwrap_or_else(|| {
            "Updates are checked only when you choose Check for updates.".into()
        });
        let state = Arc::new(Mutex::new(
            json!({"supported":supported,"state":if supported{"idle"}else{"unsupported"},"message":message,"currentVersion":app.package_info().version.to_string(),"availableVersion":null,"releaseDate":null,"progress":null,"checkedAt":null}),
        ));
        Self {
            app,
            state,
            endpoint: url::Url::parse(endpoint).ok(),
            pubkey: key,
            pending: None,
            downloaded: None,
        }
    }
    pub fn status(&self) -> Value {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn ensure_supported(&self) -> Result<(), String> {
        let status = self.status();
        if status["supported"] != true {
            return Err(status["message"]
                .as_str()
                .unwrap_or("Updates are unavailable in this build.")
                .to_string());
        }
        Ok(())
    }
    fn error(&self, message: String) -> String {
        let message = message.chars().take(1200).collect::<String>();
        publish(
            &self.app,
            &self.state,
            json!({"state":"error","message":message,"progress":null}),
        );
        message
    }
    pub async fn check(&mut self) -> Result<Value, String> {
        self.ensure_supported()?;
        let state = self.status();
        if state["state"] == "downloaded" || state["state"] == "installing" {
            return Ok(state);
        }
        publish(
            &self.app,
            &self.state,
            json!({"state":"checking","message":"Checking the published release feed…","availableVersion":null,"releaseDate":null,"progress":null}),
        );
        self.pending = None;
        self.downloaded = None;
        let result = async {
            let updater = self
                .app
                .updater_builder()
                // The shell flushes the workspace and pauses cloud sync before
                // installation. Tauri's default hook hides windows and destroys
                // tray/resources before ShellExecute, even when launching fails.
                // Keep the UI intact on failure; a successful launch immediately
                // exits this process and Windows releases its UI resources.
                .on_before_exit(|| {})
                .pubkey(self.pubkey.clone())
                .endpoints(vec![self
                    .endpoint
                    .clone()
                    .ok_or("The packaged update endpoint is invalid.")?])
                .map_err(|e| e.to_string())?
                .timeout(Duration::from_secs(60))
                .build()
                .map_err(|e| e.to_string())?;
            updater.check().await.map_err(|e| e.to_string())
        }
        .await;
        match result {
            Ok(Some(update)) => {
                if update.download_url.scheme() != "https"
                    || !update.download_url.username().is_empty()
                    || update.download_url.password().is_some()
                    || update.signature.trim().is_empty()
                {
                    return Err(self.error(
                        "The update feed returned an unsigned package or an insecure download URL."
                            .into(),
                    ));
                }
                let checked =
                    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                let date = update
                    .date
                    .and_then(|d| {
                        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
                            (d.unix_timestamp_nanos() / 1_000_000) as i64,
                        )
                    })
                    .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
                let snapshot = publish(
                    &self.app,
                    &self.state,
                    json!({"state":"available","message":format!("Version {} is available. Choose Download to continue.",update.version),"availableVersion":update.version,"releaseDate":date,"progress":null,"checkedAt":checked}),
                );
                self.pending = Some(update);
                Ok(snapshot)
            }
            Ok(None) => Ok(publish(
                &self.app,
                &self.state,
                json!({"state":"current","message":"No newer published release is available.","availableVersion":null,"releaseDate":null,"progress":null,"checkedAt":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis,true)}),
            )),
            Err(error) => Err(self.error(error)),
        }
    }
    pub async fn download(&mut self) -> Result<Value, String> {
        self.ensure_supported()?;
        if self.status()["state"] != "available" {
            return Err("Check for an available update before downloading.".into());
        }
        let update = self
            .pending
            .as_ref()
            .ok_or("Check for an available update before downloading.")?
            .clone();
        self.downloaded = None;
        publish(
            &self.app,
            &self.state,
            json!({"state":"downloading","message":"Downloading and verifying the update…","progress":{"percent":0}}),
        );
        let app = self.app.clone();
        let state = self.state.clone();
        let began = Instant::now();
        let mut transferred = 0u64;
        let result=update.download(move|chunk,total|{transferred=transferred.saturating_add(chunk as u64);let seconds=began.elapsed().as_secs_f64();let percent=total.filter(|t|*t>0).map(|t|(transferred as f64/t as f64*100.).clamp(0.,100.)).unwrap_or(0.);publish(&app,&state,json!({"state":"downloading","message":"Downloading and verifying the update…","progress":{"percent":percent,"transferred":transferred,"total":total.unwrap_or(0),"bytesPerSecond":if seconds>0.{transferred as f64/seconds}else{0.}}}));},||{}).await;
        match result {
            Ok(bytes) => {
                self.downloaded = Some(bytes);
                Ok(publish(
                    &self.app,
                    &self.state,
                    json!({"state":"downloaded","message":"The verified update is ready. Choose Restart and install when your work is saved.","availableVersion":update.version,"progress":{"percent":100}}),
                ))
            }
            Err(error) => Err(self.error(error.to_string())),
        }
    }
    /// The command boundary must flush the workspace before invoking this method.
    pub async fn install(&mut self) -> Result<Value, String> {
        self.ensure_supported()?;
        if self.status()["state"] != "downloaded" || self.downloaded.is_none() {
            return Err("Download and verify an update before installing.".into());
        }
        let update = self
            .pending
            .as_ref()
            .ok_or("Download and verify an update before installing.")?;
        let bytes = self
            .downloaded
            .as_ref()
            .ok_or("Download and verify an update before installing.")?;
        publish(
            &self.app,
            &self.state,
            json!({"state":"installing","message":"Restarting to install the verified update…"}),
        );
        match update.install(bytes) {
            Ok(()) => Ok(self.status()),
            Err(error) => Err(self.error(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trusted_release_gate_rejects_portable_development_unsigned_and_insecure_config() {
        assert!(support_error(
            true,
            true,
            true,
            true,
            "key",
            "https://example.com/latest.json",
            true
        )
        .is_some());
        assert!(support_error(
            true,
            false,
            false,
            true,
            "key",
            "https://example.com/latest.json",
            true
        )
        .is_some());
        assert!(support_error(
            true,
            false,
            true,
            false,
            "key",
            "https://example.com/latest.json",
            true
        )
        .is_some());
        assert!(support_error(
            true,
            false,
            true,
            true,
            "",
            "https://example.com/latest.json",
            true
        )
        .is_some());
        assert!(support_error(
            true,
            false,
            true,
            true,
            "key",
            "http://example.com/latest.json",
            true
        )
        .is_some());
        assert!(support_error(
            true,
            false,
            true,
            true,
            "key",
            "https://user:secret@example.com/latest.json",
            true
        )
        .is_some());
        assert!(support_error(
            true,
            false,
            true,
            true,
            "key",
            "https://example.com/latest.json",
            false
        )
        .is_some());
        assert!(support_error(
            true,
            false,
            true,
            true,
            "key",
            "https://example.com/latest.json",
            true
        )
        .is_none());
    }
}
