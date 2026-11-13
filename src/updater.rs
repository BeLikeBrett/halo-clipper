//! Explicit, signed release updates. Private signing key is never shipped.
use anyhow::{Context, Result};
use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};
pub const REPO: &str = "https://github.com/BeLikeBrett/halo-clipper";
const API: &str = "https://api.github.com/repos/BeLikeBrett/halo-clipper/releases/latest";
#[derive(Clone, Debug, Deserialize)]
pub struct Asset {
    pub target: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}
#[derive(Debug)]
pub enum State {
    Idle,
    Working(String),
    Current,
    Available(Manifest),
    Staged(PathBuf),
    Error(String),
}
pub struct Updater {
    pub state: State,
    receiver: Option<mpsc::Receiver<State>>,
}
impl Default for Updater {
    fn default() -> Self {
        Self {
            state: State::Idle,
            receiver: None,
        }
    }
}
fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(concat!("Halo-Clipper/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(15))
        .https_only(true)
        .build()?)
}
fn limited(client: &reqwest::blocking::Client, url: &str, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    client
        .get(url)
        .send()?
        .error_for_status()?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= limit,
        "Release metadata exceeds the size limit"
    );
    Ok(bytes)
}
pub fn verify_manifest(bytes: &[u8], signature: &[u8]) -> Result<Manifest> {
    let key = VerifyingKey::from_bytes(include_bytes!("../assets/update-public-key.bin"))?;
    let signature =
        base64::engine::general_purpose::STANDARD.decode(std::str::from_utf8(signature)?.trim())?;
    key.verify_strict(bytes, &Signature::from_slice(&signature)?)
        .context("Update signature is invalid. Nothing was installed.")?;
    let m: Manifest = serde_json::from_slice(bytes)?;
    semver::Version::parse(&m.version)?;
    for a in &m.assets {
        anyhow::ensure!(
            a.url
                .starts_with(&format!("{REPO}/releases/download/v{}/", m.version)),
            "Unexpected update source"
        );
        anyhow::ensure!(
            a.size > 0
                && a.size < 1024 * 1024 * 1024
                && a.sha256.len() == 64
                && a.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid update asset"
        );
    }
    Ok(m)
}
fn check() -> Result<State> {
    let c = client()?;
    let response = c.get(API).send()?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        anyhow::bail!("No public release is available yet.");
    }
    let mut metadata = Vec::new();
    response
        .error_for_status()?
        .take(1024 * 1024)
        .read_to_end(&mut metadata)?;
    let release: Release = serde_json::from_slice(&metadata)?;
    anyhow::ensure!(
        !release.draft && !release.prerelease,
        "No stable release is available"
    );
    let version = semver::Version::parse(release.tag_name.trim_start_matches('v'))?;
    if version <= semver::Version::parse(env!("CARGO_PKG_VERSION"))? {
        return Ok(State::Current);
    }
    let root = format!("{REPO}/releases/download/v{version}");
    let m = verify_manifest(
        &limited(&c, &format!("{root}/update.json"), 128 * 1024)?,
        &limited(&c, &format!("{root}/update.json.sig"), 4096)?,
    )?;
    anyhow::ensure!(
        m.version == version.to_string(),
        "Release and signed manifest versions disagree"
    );
    Ok(State::Available(m))
}
pub fn target() -> &'static str {
    if cfg!(windows) {
        "windows-x86_64"
    } else {
        "linux-x86_64"
    }
}
fn stage(m: Manifest) -> Result<State> {
    anyhow::ensure!(
        cfg!(target_arch = "x86_64"),
        "This CPU architecture requires a manual update"
    );
    let a = m
        .assets
        .iter()
        .find(|a| a.target == target())
        .context("No update for this platform")?;
    let parent = crate::platform::data_dir().join("updates");
    std::fs::create_dir_all(&parent)?;
    let path = parent.join(format!(
        "halo-clipper-{}{}",
        m.version,
        if cfg!(windows) {
            "-setup.exe"
        } else {
            "-linux"
        }
    ));
    let partial = path.with_extension("download");
    let result = (|| -> Result<()> {
        let mut response = client()?.get(&a.url).send()?.error_for_status()?;
        let mut file = std::fs::File::create(&partial)?;
        let mut hash = Sha256::new();
        let mut size = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            let n = response.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            size += n as u64;
            anyhow::ensure!(size <= a.size, "Update exceeds signed size");
            hash.update(&buffer[..n]);
            file.write_all(&buffer[..n])?;
        }
        file.sync_all()?;
        anyhow::ensure!(
            size == a.size && format!("{:x}", hash.finalize()).eq_ignore_ascii_case(&a.sha256),
            "Update checksum mismatch. Nothing was installed."
        );
        std::fs::rename(&partial, &path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result?;
    Ok(State::Staged(path))
}
impl Updater {
    fn start(&mut self, label: &str, work: impl FnOnce() -> Result<State> + Send + 'static) {
        if self.receiver.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.state = State::Working(label.into());
        std::thread::spawn(move || {
            let state = work().unwrap_or_else(|e| {
                log::error!("updater: {e:#}");
                State::Error(format!("{e:#}"))
            });
            let _ = tx.send(state);
        });
    }
    pub fn check(&mut self) {
        self.start("Checking for updates…", check);
    }
    pub fn download(&mut self, m: Manifest) {
        self.start("Downloading and verifying update…", move || stage(m));
    }
    pub fn poll(&mut self) {
        if let Some(rx) = &self.receiver {
            match rx.try_recv() {
                Ok(state) => {
                    self.state = state;
                    self.receiver = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.state = State::Error("Update worker stopped. Try again.".into());
                    self.receiver = None;
                }
                _ => {}
            }
        }
    }
}
/// The downloaded executable must load its native libraries and initialize
/// mpv before the current executable or its known-good backup is touched.
#[cfg(not(windows))]
fn prepare_linux_update(exe: &Path, path: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let next = exe.with_extension("update");
    let backup = exe.with_extension("previous");
    std::fs::copy(path, &next).context("Application folder is not writable")?;
    std::fs::set_permissions(&next, std::fs::Permissions::from_mode(0o755))?;
    if let Err(error) = verify_linux_candidate(&next) {
        let _ = std::fs::remove_file(&next);
        return Err(error);
    }
    std::fs::copy(exe, &backup)?;
    std::fs::rename(&next, exe)?;
    Ok(backup)
}

#[cfg(not(windows))]
fn verify_linux_candidate(path: &Path) -> Result<()> {
    use std::{process::Stdio, time::Instant};
    let mut child = crate::platform::command(path)
        .arg("--self-check")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("The update cannot start. The current application was kept.")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait()? {
            let output = child.wait_with_output()?;
            anyhow::ensure!(
                status.success(),
                "The update failed its startup check. The current application was kept.\n{}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "The update's startup check timed out. The current application was kept."
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub fn install(path: &Path) -> Result<()> {
    let exe = std::env::current_exe()?;
    #[cfg(windows)]
    {
        // Constant script; paths travel through environment values, never script text.
        crate::platform::command("powershell.exe").args(["-NoProfile","-NonInteractive","-Command",r#"$ErrorActionPreference='Stop'; 'Waiting for Halo Clipper to close' | Set-Content -LiteralPath $env:HALO_UPDATE_LOG; Wait-Process -Id ([int]$env:HALO_UPDATE_PID) -Timeout 180 -ErrorAction SilentlyContinue; if (Get-Process -Id ([int]$env:HALO_UPDATE_PID) -ErrorAction SilentlyContinue) { exit 1 }; $arguments='/S /D='+$env:HALO_UPDATE_DIR; $p=Start-Process -FilePath $env:HALO_UPDATE_FILE -ArgumentList $arguments -Wait -PassThru; ('Installer exit code: '+$p.ExitCode) | Add-Content -LiteralPath $env:HALO_UPDATE_LOG; if ($p.ExitCode -eq 0) { Start-Process -FilePath $env:HALO_UPDATE_APP; Remove-Item -LiteralPath $env:HALO_UPDATE_FILE -ErrorAction SilentlyContinue }"#])
            .env("HALO_UPDATE_LOG",crate::diagnostics::log_dir().join("update-install.log")).env("HALO_UPDATE_PID",std::process::id().to_string()).env("HALO_UPDATE_FILE",path).env("HALO_UPDATE_DIR",exe.parent().context("Application has no parent folder")?).env("HALO_UPDATE_APP",&exe).spawn()?;
    }
    #[cfg(not(windows))]
    {
        anyhow::ensure!(
            !exe.starts_with("/usr") && !exe.starts_with("/opt"),
            "This is a system-managed install. Update through your package manager."
        );
        let backup = prepare_linux_update(&exe, path)?;
        let launch = (|| -> Result<()> {
            let mut child = crate::platform::command(&exe).spawn()?;
            // An immediate loader/window failure must not close the old UI.
            for _ in 0..20 {
                if let Some(status) = child.try_wait()? {
                    anyhow::bail!("The updated application exited during startup ({status}).");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(())
        })();
        if let Err(error) = launch {
            let restore = exe.with_extension("restore");
            std::fs::copy(&backup, &restore).context("Could not restore the previous version")?;
            std::fs::rename(&restore, &exe)?;
            return Err(
                error.context("The previous version was restored; this application will stay open")
            );
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(windows))]
    #[test]
    fn failed_linux_startup_keeps_current_binary_and_good_backup() {
        let dir = std::env::temp_dir().join(format!("halo-update-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let current = dir.join("halo-clipper");
        let backup = current.with_extension("previous");
        let candidate = dir.join("candidate");
        std::fs::write(&current, b"working current").unwrap();
        std::fs::write(&backup, b"working backup").unwrap();
        std::fs::write(
            &candidate,
            b"#!/bin/sh\necho 'missing libmpv.so.2' >&2\nexit 127\n",
        )
        .unwrap();
        let error = prepare_linux_update(&current, &candidate).unwrap_err();
        assert!(format!("{error:#}").contains("missing libmpv.so.2"));
        assert_eq!(std::fs::read(&current).unwrap(), b"working current");
        assert_eq!(std::fs::read(&backup).unwrap(), b"working backup");
        assert!(!current.with_extension("update").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reject_unsigned_or_modified_manifest() {
        assert!(verify_manifest(br#"{"version":"9.0.0","assets":[]}"#, b"AAAA").is_err());
    }
    #[test]
    fn accept_signed_fixture_and_reject_tampering() {
        let bytes = include_bytes!("../tests/fixtures/update.json");
        let signature = include_bytes!("../tests/fixtures/update.json.sig");
        assert_eq!(verify_manifest(bytes, signature).unwrap().version, "0.7.0");
        let mut changed = bytes.to_vec();
        changed[12] = b'9';
        assert!(verify_manifest(&changed, signature).is_err());
    }
    #[test]
    fn stable_version_order() {
        assert!(
            semver::Version::parse("0.10.0").unwrap() > semver::Version::parse("0.9.0").unwrap()
        );
    }
}
