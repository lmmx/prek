use std::env::consts::EXE_EXTENSION;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use itertools::Itertools;
use prek_consts::env_vars::EnvVars;
use tracing::{debug, trace, warn};

use crate::fs::LockedFile;
use crate::languages::rust::RustRequest;
use crate::languages::rust::version::RustVersion;
use crate::process::Cmd;
use crate::store::Store;

pub(crate) struct RustResult {
    path: PathBuf,
    version: RustVersion,
    from_system: bool,
    /// The rustup home directory used for this installation.
    /// None if using system rust without rustup.
    rustup_home: Option<PathBuf>,
}

impl Display for RustResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.path.display(), self.version)?;
        Ok(())
    }
}

/// Override the Rust binary name for testing.
static RUST_BINARY_NAME: LazyLock<String> = LazyLock::new(|| {
    if let Ok(name) = EnvVars::var(EnvVars::PREK_INTERNAL__RUST_BINARY_NAME) {
        name
    } else {
        "rustc".to_string()
    }
});

/// Override the Rustup binary name for testing.
static RUSTUP_BINARY_NAME: LazyLock<String> = LazyLock::new(|| {
    if let Ok(name) = EnvVars::var(EnvVars::PREK_INTERNAL__RUSTUP_BINARY_NAME) {
        name
    } else {
        "rustup".to_string()
    }
});

impl RustResult {
    fn new(path: PathBuf, from_system: bool, rustup_home: Option<PathBuf>) -> Self {
        Self {
            path,
            from_system,
            version: RustVersion::default(),
            rustup_home,
        }
    }

    pub(crate) fn bin(&self) -> &Path {
        &self.path
    }

    pub(crate) fn version(&self) -> &RustVersion {
        &self.version
    }

    pub(crate) fn is_from_system(&self) -> bool {
        self.from_system
    }

    pub(crate) fn rustup_home(&self) -> Option<&Path> {
        self.rustup_home.as_deref()
    }

    pub(crate) fn cmd(&self, summary: &str) -> Cmd {
        let mut cmd = Cmd::new(&self.path, summary);
        cmd.env(EnvVars::RUSTUP_AUTO_INSTALL, "0");
        if let Some(rustup_home) = &self.rustup_home {
            cmd.env(EnvVars::RUSTUP_HOME, rustup_home);
        }
        cmd
    }

    pub(crate) fn with_version(mut self, version: RustVersion) -> Self {
        self.version = version;
        self
    }

    pub(crate) async fn fill_version(mut self) -> Result<Self> {
        let output = self
            .cmd("rustc version")
            .arg("--version")
            .check(true)
            .output()
            .await?;

        // e.g. "rustc 1.70.0 (90c541806 2023-05-31)"
        let version_str = str::from_utf8(&output.stdout)?;
        let version_str = version_str.split_ascii_whitespace().nth(1).ok_or_else(|| {
            anyhow::anyhow!("Failed to parse Rust version from output: {version_str}")
        })?;

        let version = RustVersion::from_str(version_str)?;

        self.version = version;

        Ok(self)
    }

    pub(crate) async fn fill_version_with_toolchain(mut self, toolchain: &str) -> Result<Self> {
        let mut cmd = self.cmd("rustc version");
        cmd.arg("--version")
            .env(EnvVars::RUSTUP_TOOLCHAIN, toolchain);

        let output = cmd.check(true).output().await?;

        // e.g. "rustc 1.70.0 (90c541806 2023-05-31)"
        let version_str = str::from_utf8(&output.stdout)?;
        let version_str = version_str.split_ascii_whitespace().nth(1).ok_or_else(|| {
            anyhow::anyhow!("Failed to parse Rust version from output: {version_str}")
        })?;

        let version = RustVersion::from_str(version_str)?;
        self.version = version;

        Ok(self)
    }
}

/// Information about a rustup installation.
#[derive(Debug)]
pub(crate) struct RustupInfo {
    /// Path to the rustup binary.
    pub(crate) bin: PathBuf,
    /// `RUSTUP_HOME` directory.
    pub(crate) home: PathBuf,
    /// `CARGO_HOME` directory (where rustup and cargo binaries live).
    pub(crate) cargo_home: PathBuf,
    /// Whether this is a system rustup installation.
    #[allow(dead_code)]
    pub(crate) from_system: bool,
}

impl RustupInfo {
    pub(crate) fn cmd(&self, summary: &str) -> Cmd {
        let mut cmd = Cmd::new(&self.bin, summary);
        cmd.env(EnvVars::RUSTUP_HOME, &self.home);
        cmd.env(EnvVars::CARGO_HOME, &self.cargo_home);
        cmd.env(EnvVars::RUSTUP_AUTO_INSTALL, "0");
        cmd
    }

    /// Get the path to rustc proxy.
    ///
    /// This returns the rustc in `CARGO_HOME/bin` which is a rustup proxy,
    /// which will delegate to the correct toolchain when `RUSTUP_TOOLCHAIN` is set.
    pub(crate) fn rustc_proxy_path(&self) -> PathBuf {
        self.cargo_home
            .join("bin")
            .join("rustc")
            .with_extension(EXE_EXTENSION)
    }
}

pub(crate) struct RustInstaller {
    /// Root directory for Rust tools: `$PREK_HOME/tools/rust`
    root: PathBuf,
}

impl RustInstaller {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Returns the shared `RUSTUP_HOME` directory: `$PREK_HOME/tools/rustup`
    fn shared_rustup_home(&self) -> PathBuf {
        // root is $PREK_HOME/tools/rust, so parent is $PREK_HOME/tools
        self.root
            .parent()
            .expect("rust root should have parent")
            .join("rustup")
    }

    /// Returns the `CARGO_HOME` directory for managed rustup: `$PREK_HOME/tools/rust/cargo`
    fn managed_cargo_home(&self) -> PathBuf {
        self.root.join("cargo")
    }

    pub(crate) async fn install(
        &self,
        _store: &Store,
        request: &RustRequest,
        allows_download: bool,
    ) -> Result<RustResult> {
        fs_err::tokio::create_dir_all(&self.root).await?;
        let _lock = LockedFile::acquire(self.root.join(".lock"), "rust").await?;

        // First, try to find or install rustup
        let rustup = self.find_or_install_rustup(allows_download).await?;

        // Now find or install the requested toolchain
        self.install_toolchain(&rustup, request, allows_download)
            .await
    }

    /// Find system rustup or install a managed one.
    async fn find_or_install_rustup(&self, allows_download: bool) -> Result<RustupInfo> {
        // Check for system rustup first
        if let Some(rustup) = self.find_system_rustup().await? {
            trace!(bin = %rustup.bin.display(), "Using system rustup");
            return Ok(rustup);
        }

        // Check for already-installed managed rustup
        let managed_cargo_home = self.managed_cargo_home();
        let managed_rustup_bin = managed_cargo_home
            .join("bin")
            .join("rustup")
            .with_extension(EXE_EXTENSION);

        if managed_rustup_bin.exists() {
            trace!(bin = %managed_rustup_bin.display(), "Using managed rustup");
            return Ok(RustupInfo {
                bin: managed_rustup_bin,
                home: self.shared_rustup_home(),
                cargo_home: managed_cargo_home,
                from_system: false,
            });
        }

        if !allows_download {
            anyhow::bail!("No rustup found and downloads are disabled");
        }

        // Install rustup
        self.install_rustup().await
    }

    /// Find system rustup installation.
    async fn find_system_rustup(&self) -> Result<Option<RustupInfo>> {
        let rustup_paths = match which::which_all(&*RUSTUP_BINARY_NAME) {
            Ok(paths) => paths,
            Err(e) => {
                debug!("No rustup executables found in PATH: {}", e);
                return Ok(None);
            }
        };

        for rustup_path in rustup_paths {
            // Verify it works by running `rustup --version`
            let output = Cmd::new(&rustup_path, "rustup version check")
                .arg("--version")
                .check(false)
                .output()
                .await;

            match output {
                Ok(output) if output.status.success() => {
                    // Get the cargo home from the rustup path
                    // rustup is typically at $CARGO_HOME/bin/rustup
                    let cargo_home = rustup_path
                        .parent() // bin/
                        .and_then(|p| p.parent()) // $CARGO_HOME
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| {
                            // Fallback to ~/.cargo
                            EnvVars::var_os(EnvVars::HOME)
                                .map(|h| PathBuf::from(h).join(".cargo"))
                                .unwrap_or_else(|| PathBuf::from(".cargo"))
                        });

                    // Use our shared RUSTUP_HOME even with system rustup
                    // This ensures toolchains are installed in a consistent location
                    return Ok(Some(RustupInfo {
                        bin: rustup_path,
                        home: self.shared_rustup_home(),
                        cargo_home,
                        from_system: true,
                    }));
                }
                Ok(_) => {
                    debug!(path = %rustup_path.display(), "System rustup check failed");
                }
                Err(e) => {
                    debug!(path = %rustup_path.display(), error = %e, "Failed to run system rustup");
                }
            }
        }

        Ok(None)
    }

    /// Install rustup into our managed location.
    async fn install_rustup(&self) -> Result<RustupInfo> {
        let cargo_home = self.managed_cargo_home();
        let rustup_home = self.shared_rustup_home();

        fs_err::tokio::create_dir_all(&cargo_home).await?;
        fs_err::tokio::create_dir_all(&rustup_home).await?;

        // Download rustup-init to a temporary location
        let rustup_init_dir = tempfile::tempdir()?;

        let url = if cfg!(windows) {
            "https://win.rustup.rs/x86_64"
        } else {
            "https://sh.rustup.rs"
        };

        let resp = crate::languages::REQWEST_CLIENT
            .get(url)
            .send()
            .await
            .context("Failed to download rustup-init")?;

        if !resp.status().is_success() {
            anyhow::bail!("Failed to download rustup-init: {}", resp.status());
        }

        let rustup_init = rustup_init_dir
            .path()
            .join("rustup-init")
            .with_extension(EXE_EXTENSION);
        fs_err::tokio::write(&rustup_init, resp.bytes().await?).await?;
        make_executable(&rustup_init)?;

        // Install rustup with no default toolchain
        Cmd::new(&rustup_init, "install rustup")
            .args([
                "-y",
                "--quiet",
                "--no-modify-path",
                "--default-toolchain",
                "none",
            ])
            .env(EnvVars::CARGO_HOME, &cargo_home)
            .env(EnvVars::RUSTUP_HOME, &rustup_home)
            .check(true)
            .output()
            .await?;

        let rustup_bin = cargo_home
            .join("bin")
            .join("rustup")
            .with_extension(EXE_EXTENSION);

        Ok(RustupInfo {
            bin: rustup_bin,
            home: rustup_home,
            cargo_home,
            from_system: false,
        })
    }

    /// Install the requested toolchain using rustup.
    async fn install_toolchain(
        &self,
        rustup: &RustupInfo,
        request: &RustRequest,
        allows_download: bool,
    ) -> Result<RustResult> {
        // First check if we already have an installed toolchain that matches
        if let Some(result) = self.find_installed_toolchain(rustup, request).await? {
            trace!(%result, "Found installed toolchain matching request");
            return Ok(result);
        }

        // For system-only requests, also check system rust not managed by our rustup
        if !allows_download {
            if let Some(rust) = self.find_system_rust(request).await? {
                trace!(%rust, "Using system rust");
                return Ok(rust);
            }
            anyhow::bail!("No suitable Rust version found and downloads are disabled");
        }

        // Resolve the toolchain name and install it
        let toolchain = self.resolve_toolchain(request).await?;
        self.install_toolchain_with_rustup(rustup, &toolchain)
            .await?;

        // Return the result - use the rustc proxy which will delegate to the correct toolchain
        let rustc_path = rustup.rustc_proxy_path();

        RustResult::new(rustc_path, false, Some(rustup.home.clone()))
            .fill_version_with_toolchain(&toolchain)
            .await
    }

    /// Find an installed toolchain that matches the request.
    async fn find_installed_toolchain(
        &self,
        rustup: &RustupInfo,
        request: &RustRequest,
    ) -> Result<Option<RustResult>> {
        let toolchains_dir = rustup.home.join("toolchains");

        if !toolchains_dir.exists() {
            return Ok(None);
        }

        let installed = fs_err::read_dir(&toolchains_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(e) => {
                    warn!(?e, "Failed to read toolchain entry");
                    None
                }
            })
            .filter(|entry| entry.file_type().is_ok_and(|f| f.is_dir()))
            .map(|entry| {
                let dir_name = entry.file_name();
                let dir_str = dir_name.to_string_lossy().to_string();
                // Parse version from toolchain name (e.g., "1.70.0-x86_64-unknown-linux-gnu" -> "1.70.0")
                let version_str = dir_str.split('-').next().unwrap_or(&dir_str);
                let version = RustVersion::from_str(version_str).ok();
                (dir_str, version, entry.path())
            })
            .sorted_unstable_by(|(_, a, _), (_, b, _)| {
                match (a, b) {
                    (Some(a), Some(b)) => b.cmp(a), // newest first
                    _ => std::cmp::Ordering::Equal,
                }
            });

        for (name, version, path) in installed {
            let matches = match (request, &version) {
                (RustRequest::Channel(ch), _) => name.starts_with(ch),
                (_, Some(v)) => request.matches(v, Some(&path)),
                _ => false,
            };

            if matches {
                trace!(name = %name, "Found matching installed toolchain");
                // Use the proxy path, not the direct toolchain path
                let rustc_path = rustup.rustc_proxy_path();
                let result = RustResult::new(rustc_path, false, Some(rustup.home.clone()));

                return match result.fill_version_with_toolchain(&name).await {
                    Ok(result) => Ok(Some(result)),
                    Err(e) => {
                        warn!(?e, name = %name, "Failed to get version for installed toolchain");
                        continue;
                    }
                };
            }
        }

        Ok(None)
    }

    /// Find system rust that's not managed by rustup (e.g., distro packages).
    async fn find_system_rust(&self, rust_request: &RustRequest) -> Result<Option<RustResult>> {
        let rust_paths = match which::which_all(&*RUST_BINARY_NAME) {
            Ok(paths) => paths,
            Err(e) => {
                debug!("No rustc executables found in PATH: {}", e);
                return Ok(None);
            }
        };

        for rust_path in rust_paths {
            // Skip rustc that's a rustup proxy (we handle those via rustup)
            if is_rustup_proxy(&rust_path) {
                continue;
            }

            match RustResult::new(rust_path.clone(), true, None)
                .fill_version()
                .await
            {
                Ok(rust) => {
                    if rust_request.matches(&rust.version, Some(&rust.path)) {
                        trace!(%rust, "Found matching system rust");
                        return Ok(Some(rust));
                    }
                    trace!(%rust, "System rust does not match requested version");
                }
                Err(e) => {
                    warn!(?e, "Failed to get version for system rust");
                }
            }
        }

        debug!(
            ?rust_request,
            "No system rust matches the requested version"
        );
        Ok(None)
    }

    /// Install a toolchain using rustup.
    async fn install_toolchain_with_rustup(
        &self,
        rustup: &RustupInfo,
        toolchain: &str,
    ) -> Result<()> {
        rustup
            .cmd("install toolchain")
            .args(["toolchain", "install", "--no-self-update", toolchain])
            .check(true)
            .output()
            .await?;

        Ok(())
    }

    /// Resolve a version request to a toolchain name.
    async fn resolve_toolchain(&self, req: &RustRequest) -> Result<String> {
        match req {
            RustRequest::Any => Ok("stable".to_string()),
            RustRequest::Channel(ch) => Ok(ch.clone()),
            RustRequest::Path(p) => Ok(p.to_string_lossy().to_string()),

            RustRequest::Major(_)
            | RustRequest::MajorMinor(_, _)
            | RustRequest::MajorMinorPatch(_, _, _)
            | RustRequest::Range(_, _) => {
                let output = crate::git::git_cmd("list rust tags")?
                    .arg("ls-remote")
                    .arg("--tags")
                    .arg("https://github.com/rust-lang/rust")
                    .output()
                    .await?
                    .stdout;
                let versions: Vec<RustVersion> = str::from_utf8(&output)?
                    .lines()
                    .filter_map(|line| {
                        let tag = line.split('\t').nth(1)?;
                        let tag = tag.strip_prefix("refs/tags/")?;
                        RustVersion::from_str(tag).ok()
                    })
                    .sorted_unstable_by(|a, b| b.cmp(a))
                    .collect();

                let version = versions
                    .into_iter()
                    .find(|version| req.matches(version, None))
                    .context("Version not found on remote")?;
                Ok(version.to_string())
            }
        }
    }
}

pub(crate) fn bin_dir(env_path: &Path) -> PathBuf {
    env_path.join("bin")
}

/// Check if a rustc binary is a rustup proxy.
fn is_rustup_proxy(rustc_path: &Path) -> bool {
    // Rustup proxies are typically in ~/.cargo/bin or similar
    // and are small shim executables
    if let Some(parent) = rustc_path.parent() {
        if let Some(parent_name) = parent.file_name() {
            if parent_name == "bin" {
                if let Some(grandparent) = parent.parent() {
                    if let Some(gp_name) = grandparent.file_name() {
                        // Check if it's in a cargo home directory
                        if gp_name == ".cargo" || gp_name == "cargo" {
                            return true;
                        }
                    }
                }
            }
        }
    }

    // Also check if rustup is in the same directory
    rustc_path
        .parent()
        .map(|p| p.join("rustup").with_extension(EXE_EXTENSION).exists())
        .unwrap_or(false)
}

#[cfg(unix)]
fn make_executable(filename: impl AsRef<Path>) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(filename.as_ref())?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    std::fs::set_permissions(filename.as_ref(), permissions)?;

    Ok(())
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn make_executable(_filename: impl AsRef<Path>) -> std::io::Result<()> {
    Ok(())
}
