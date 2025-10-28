use std::str::FromStr;

use crate::hook::InstallInfo;

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("Invalid `language_version` value: `{0}`")]
    InvalidVersion(String),
}

#[derive(Debug, Clone)]
pub(crate) struct RustVersion(semver::Version);

impl Default for RustVersion {
    fn default() -> Self {
        RustVersion(semver::Version::new(0, 0, 0))
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum RustRequest {
    Any,
}

impl FromStr for RustRequest {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() || s == "default" || s == "system" {
            Ok(RustRequest::Any)
        } else {
            Err(Error::InvalidVersion(s.to_string()))
        }
    }
}

impl RustRequest {
    pub(crate) fn is_any(&self) -> bool {
        matches!(self, RustRequest::Any)
    }

    pub(crate) fn satisfied_by(&self, _install_info: &InstallInfo) -> bool {
        true
    }
}
