use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Rust;

impl LanguageImpl for Rust {
    async fn install(
        &self,
        hook: Arc<Hook>,
        _store: &Store,
        _reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        Ok(InstalledHook::NoNeedInstall(hook))
    }

    async fn check_health(&self, _info: &InstallInfo) -> Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        _hook: &InstalledHook,
        _filenames: &[&Path],
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        anyhow::bail!("Rust language support not yet implemented")
    }
}
