//! Unified configuration settings for prek.
//!
//! Settings are resolved from multiple sources with the following precedence (highest to lowest):
//! 1. CLI flags (handled by clap, merged separately)
//! 2. `pyproject.toml` `[tool.prek]` section
//! 3. Environment variables (`PREK_*`)
//! 4. Built-in defaults
#![allow(clippy::result_large_err)]

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use figment::providers::Serialized;
use figment::{Figment, Profile, Provider, value::Map};
use prek_consts::env_vars::EnvVars;
use serde::{Deserialize, Serialize};

/// Global settings instance, initialized lazily.
///
/// Call `Settings::init()` early in main to set the working directory,
/// or it will default to the current directory.
static SETTINGS: LazyLock<RwLock<Settings>> = LazyLock::new(|| RwLock::new(Settings::default()));

/// Check if settings have been initialized
static INITIALIZED: LazyLock<RwLock<bool>> = LazyLock::new(|| RwLock::new(false));

/// Prek configuration settings.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[allow(clippy::struct_excessive_bools)]
pub struct Settings {
    /// Hook IDs to skip (equivalent to `PREK_SKIP`).
    #[serde(default)]
    pub skip: Vec<String>,

    /// Override the prek data directory (equivalent to `PREK_HOME`).
    pub home: Option<PathBuf>,

    /// Control colored output: "auto", "always", or "never" (equivalent to `PREK_COLOR`).
    pub color: Option<ColorChoice>,

    /// Allow running without a `.pre-commit-config.yaml` (equivalent to `PREK_ALLOW_NO_CONFIG`).
    #[serde(default)]
    pub allow_no_config: bool,

    /// Disable parallelism for installs and runs (equivalent to `PREK_NO_CONCURRENCY`).
    #[serde(default)]
    pub no_concurrency: bool,

    /// Disable Rust-native built-in hooks (equivalent to `PREK_NO_FAST_PATH`).
    #[serde(default)]
    pub no_fast_path: bool,

    /// Control how uv is installed (equivalent to `PREK_UV_SOURCE`).
    pub uv_source: Option<String>,

    /// Use system's trusted store instead of bundled roots (equivalent to `PREK_NATIVE_TLS`).
    #[serde(default)]
    pub native_tls: bool,

    /// Container runtime to use: "auto", "docker", or "podman" (equivalent to `PREK_CONTAINER_RUNTIME`).
    pub container_runtime: Option<ContainerRuntime>,
}

/// Color output choice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ColorChoice {
    /// Enables colored output only when the output is going to a terminal or TTY with support.
    #[default]
    Auto,
    /// Enables colored output regardless of the detected environment.
    Always,
    /// Disables colored output.
    Never,
}

impl From<ColorChoice> for anstream::ColorChoice {
    fn from(value: ColorChoice) -> Self {
        match value {
            ColorChoice::Auto => Self::Auto,
            ColorChoice::Always => Self::Always,
            ColorChoice::Never => Self::Never,
        }
    }
}

impl std::fmt::Display for ColorChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Always => write!(f, "always"),
            Self::Never => write!(f, "never"),
        }
    }
}

/// Container runtime choice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ContainerRuntime {
    /// Auto-detect available runtime.
    #[default]
    Auto,
    /// Use Docker.
    Docker,
    /// Use Podman.
    Podman,
}

impl std::fmt::Display for ContainerRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Docker => write!(f, "docker"),
            Self::Podman => write!(f, "podman"),
        }
    }
}

/// Wrapper to extract `[tool.prek]` from pyproject.toml
#[derive(Debug, Deserialize)]
struct PyProjectToml {
    tool: Option<PyProjectTool>,
}

#[derive(Debug, Deserialize)]
struct PyProjectTool {
    prek: Option<Settings>,
}

/// Custom provider that extracts `[tool.prek]` from a TOML file
struct PyProjectProvider {
    path: PathBuf,
}

impl PyProjectProvider {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl Provider for PyProjectProvider {
    fn metadata(&self) -> figment::Metadata {
        figment::Metadata::named("pyproject.toml [tool.prek]")
            .source(self.path.display().to_string())
    }

    fn data(&self) -> Result<Map<Profile, figment::value::Dict>, figment::Error> {
        let Ok(content) = std::fs::read_to_string(&self.path) else {
            return Ok(Map::new());
        };

        let pyproject: PyProjectToml = match toml::from_str(&content) {
            Ok(p) => p,
            Err(_) => return Ok(Map::new()),
        };

        match pyproject.tool.and_then(|t| t.prek) {
            Some(settings) => Serialized::defaults(settings).data(),
            None => Ok(Map::new()),
        }
    }
}

/// Custom provider for PREK_* environment variables that handles
/// special cases like comma-separated lists and boolean values.
struct PrekEnvProvider;

impl Provider for PrekEnvProvider {
    fn metadata(&self) -> figment::Metadata {
        figment::Metadata::named("PREK_* environment variables")
    }

    fn data(&self) -> Result<Map<Profile, figment::value::Dict>, figment::Error> {
        use figment::value::{Dict, Value};

        let mut dict = Dict::new();

        // PREK_SKIP or SKIP - comma-separated list
        if let Some(val) = EnvVars::var(EnvVars::PREK_SKIP)
            .ok()
            .or_else(|| EnvVars::var(EnvVars::SKIP).ok())
        {
            let items: Vec<Value> = val
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .map(Value::from)
                .collect();
            dict.insert("skip".into(), Value::from(items));
        }

        // PREK_HOME or PRE_COMMIT_HOME (EnvVars::var handles the fallback)
        if let Ok(val) = EnvVars::var(EnvVars::PREK_HOME) {
            dict.insert("home".into(), Value::from(val));
        }

        if let Ok(val) = EnvVars::var(EnvVars::PREK_COLOR) {
            dict.insert("color".into(), Value::from(val));
        }

        // PREK_ALLOW_NO_CONFIG - boolish values (EnvVars::var handles PRE_COMMIT fallback)
        if let Some(b) = EnvVars::var_as_bool(EnvVars::PREK_ALLOW_NO_CONFIG) {
            dict.insert("allow-no-config".into(), Value::from(b));
        }

        // PREK_NO_CONCURRENCY - boolish values (EnvVars::var handles PRE_COMMIT fallback)
        if let Some(b) = EnvVars::var_as_bool(EnvVars::PREK_NO_CONCURRENCY) {
            dict.insert("no-concurrency".into(), Value::from(b));
        }

        if let Some(b) = EnvVars::var_as_bool(EnvVars::PREK_NO_FAST_PATH) {
            dict.insert("no-fast-path".into(), Value::from(b));
        }

        if let Ok(val) = EnvVars::var(EnvVars::PREK_UV_SOURCE) {
            dict.insert("uv-source".into(), Value::from(val));
        }

        if let Some(b) = EnvVars::var_as_bool(EnvVars::PREK_NATIVE_TLS) {
            dict.insert("native-tls".into(), Value::from(b));
        }

        if let Ok(val) = EnvVars::var(EnvVars::PREK_CONTAINER_RUNTIME) {
            dict.insert("container-runtime".into(), Value::from(val));
        }

        let mut map = Map::new();
        map.insert(Profile::Default, dict);
        Ok(map)
    }
}

impl Settings {
    /// Initialize settings from the given working directory.
    ///
    /// This should be called early in `main()` before any settings are accessed.
    /// If not called, settings will use the current directory when first accessed.
    pub fn init(working_dir: &Path) -> Result<(), figment::Error> {
        let settings = Self::discover(working_dir)?;
        *SETTINGS.write().expect("settings lock poisoned") = settings;
        *INITIALIZED.write().expect("initialized lock poisoned") = true;
        Ok(())
    }

    /// Initialize settings with CLI overrides.
    ///
    /// CLI flags take highest precedence over all other sources.
    pub fn init_with_cli(
        working_dir: &Path,
        cli_overrides: CliOverrides,
    ) -> Result<(), figment::Error> {
        let figment = Self::build_figment(working_dir).merge(Serialized::defaults(cli_overrides));
        let settings: Settings = figment.extract()?;
        *SETTINGS.write().expect("settings lock poisoned") = settings;
        *INITIALIZED.write().expect("initialized lock poisoned") = true;
        Ok(())
    }

    /// Get the global settings instance.
    ///
    /// If settings haven't been initialized, this will initialize them
    /// using the current directory.
    pub fn get() -> Settings {
        // Check if we need to initialize
        {
            let init_guard = INITIALIZED.read().expect("initialized lock poisoned");
            if !*init_guard {
                drop(init_guard);
                // Try to initialize with current directory
                let cwd = std::env::current_dir().unwrap_or_default();
                let _ = Self::init(&cwd);
            }
        }
        SETTINGS.read().expect("settings lock poisoned").clone()
    }

    /// Build the figment for the given working directory.
    fn build_figment(working_dir: &Path) -> Figment {
        let mut figment = Figment::new()
            // Lowest precedence: built-in defaults
            .merge(Serialized::defaults(Settings::default()))
            // Next: environment variables (custom provider for special handling)
            .merge(PrekEnvProvider);

        // Walk up to find pyproject.toml
        let mut current = Some(working_dir);
        while let Some(dir) = current {
            let pyproject_path = dir.join("pyproject.toml");
            if pyproject_path.exists() {
                // Higher precedence: pyproject.toml [tool.prek]
                figment = figment.merge(PyProjectProvider::new(pyproject_path));
                break;
            }
            current = dir.parent();
        }

        figment
    }

    /// Discover and resolve settings from the given directory.
    fn discover(start_dir: &Path) -> Result<Self, figment::Error> {
        Self::build_figment(start_dir).extract()
    }

    /// Check if a hook should be skipped.
    pub fn should_skip(&self, hook_id: &str) -> bool {
        self.skip.iter().any(|s| s == hook_id)
    }

    /// Get the resolved color choice.
    pub fn resolved_color(&self) -> ColorChoice {
        self.color.unwrap_or_default()
    }

    /// Get the resolved container runtime.
    pub fn resolved_container_runtime(&self) -> ContainerRuntime {
        self.container_runtime.unwrap_or_default()
    }
}

/// CLI overrides that take highest precedence.
///
/// This struct contains only the fields that can be set via CLI flags.
/// Fields are all optional - `None` means "use value from other sources".
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct CliOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorChoice>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_concurrency: Option<bool>,
    // Add other CLI-settable options here as needed
}

impl CliOverrides {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn color(mut self, color: Option<ColorChoice>) -> Self {
        self.color = color;
        self
    }

    #[must_use]
    pub fn no_concurrency(mut self, value: bool) -> Self {
        if value {
            self.no_concurrency = Some(true);
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();
        assert!(settings.skip.is_empty());
        assert!(settings.home.is_none());
        assert!(settings.color.is_none());
        assert!(!settings.allow_no_config);
        assert!(!settings.no_concurrency);
    }

    #[test]
    fn test_pyproject_loading() {
        let dir = TempDir::new().unwrap();
        let pyproject = dir.path().join("pyproject.toml");

        let mut file = std::fs::File::create(&pyproject).unwrap();
        write!(
            file,
            r#"
[tool.prek]
skip = ["black", "ruff"]
no-concurrency = true
color = "always"
"#
        )
        .unwrap();

        let settings = Settings::discover(dir.path()).unwrap();

        assert_eq!(settings.skip, vec!["black", "ruff"]);
        assert!(settings.no_concurrency);
        assert_eq!(settings.color, Some(ColorChoice::Always));
    }

    #[test]
    fn test_env_var_loading() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("PREK_SKIP", "hook1,hook2");
            jail.set_env("PREK_NO_CONCURRENCY", "1");
            jail.set_env("PREK_COLOR", "never");

            let settings = Settings::discover(jail.directory())?;

            assert_eq!(settings.skip, vec!["hook1", "hook2"]);
            assert!(settings.no_concurrency);
            assert_eq!(settings.color, Some(ColorChoice::Never));

            Ok(())
        });
    }

    #[test]
    fn test_pyproject_overrides_env() {
        figment::Jail::expect_with(|jail| {
            // Set env var
            jail.set_env("PREK_COLOR", "never");

            // Create pyproject.toml with different value
            jail.create_file(
                "pyproject.toml",
                r#"
[tool.prek]
color = "always"
"#,
            )?;

            let settings = Settings::discover(jail.directory())?;

            // pyproject.toml should win
            assert_eq!(settings.color, Some(ColorChoice::Always));

            Ok(())
        });
    }

    #[test]
    fn test_cli_overrides_all() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("PREK_COLOR", "never");
            jail.create_file(
                "pyproject.toml",
                r#"
[tool.prek]
color = "auto"
"#,
            )?;

            let figment = Settings::build_figment(jail.directory()).merge(Serialized::defaults(
                CliOverrides {
                    color: Some(ColorChoice::Always),
                    ..Default::default()
                },
            ));

            let settings: Settings = figment.extract()?;

            // CLI should win
            assert_eq!(settings.color, Some(ColorChoice::Always));

            Ok(())
        });
    }

    #[test]
    fn test_walks_up_directory_tree() {
        figment::Jail::expect_with(|jail| {
            // Create pyproject.toml in parent
            jail.create_file(
                "pyproject.toml",
                r#"
[tool.prek]
skip = ["parent-hook"]
"#,
            )?;

            // Create subdirectory
            std::fs::create_dir_all(jail.directory().join("subdir/nested"))
                .map_err(|e| e.to_string())?; // Convert io::Error to String, which impls Into<figment::Error>

            let settings = Settings::discover(&jail.directory().join("subdir/nested"))?;

            assert_eq!(settings.skip, vec!["parent-hook"]);

            Ok(())
        });
    }
}
