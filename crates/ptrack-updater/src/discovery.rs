use std::fmt;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{ACCEPT, USER_AGENT};
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio_util::sync::CancellationToken;

pub const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/ro-ag/ptrack/releases/latest";
const USER_AGENT_VALUE: &str = "p-track-updater";
const MAX_METADATA_BYTES: usize = 1 << 20;
const MAX_NOTES_BYTES: usize = 32 << 10;
const MAX_MANIFEST_BYTES: u64 = 1 << 20;
const MAX_ASSET_BYTES: u64 = 512 << 20;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateError {
    DevelopmentBuild,
    InvalidRelease,
    NoUpdate,
    UnsupportedTarget,
    InvalidStage,
    InstallRefused,
    PendingStageMismatch,
    Cancelled,
    Message(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DevelopmentBuild => "updates are unavailable for development builds",
            Self::InvalidRelease => "invalid GitHub release",
            Self::NoUpdate => "no newer release is available",
            Self::UnsupportedTarget => "unsupported update target",
            Self::InvalidStage => "invalid staged update",
            Self::InstallRefused => "update installation refused",
            Self::PendingStageMismatch => "pending update belongs to another verified stage",
            Self::Cancelled => "update operation was canceled",
            Self::Message(message) => message,
        })
    }
}

impl std::error::Error for UpdateError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    pub os: String,
    pub arch: String,
}

impl Target {
    #[must_use]
    pub fn host() -> Self {
        Self {
            os: match std::env::consts::OS {
                "macos" => "darwin",
                other => other,
            }
            .to_owned(),
            arch: match std::env::consts::ARCH {
                "x86_64" => "amd64",
                "aarch64" => "arm64",
                other => other,
            }
            .to_owned(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Asset {
    pub name: String,
    pub download_url: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub version: String,
    pub tag: String,
    pub page_url: String,
    pub published_at: String,
    pub notes: String,
    pub package: Asset,
    pub checksums: Asset,
}

pub struct Client {
    endpoint: String,
    http: reqwest::Client,
    #[cfg(test)]
    test_asset_base: Option<String>,
    #[cfg(test)]
    test_asset_http: Option<reqwest::Client>,
}

impl Client {
    /// Constructs the fixed, redirect-refusing production discovery client.
    ///
    /// # Errors
    /// Returns an error when the TLS or HTTP client cannot be configured.
    pub fn new() -> Result<Self, UpdateError> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| UpdateError::Message("configure release client".to_owned()))?;
        Ok(Self {
            endpoint: LATEST_RELEASE_URL.to_owned(),
            http,
            #[cfg(test)]
            test_asset_base: None,
            #[cfg(test)]
            test_asset_http: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_endpoint(endpoint: String) -> Result<Self, UpdateError> {
        let mut value = Self::new()?;
        value.endpoint = endpoint;
        Ok(value)
    }

    #[cfg(test)]
    pub(crate) fn with_test_asset_server(base: String) -> Result<Self, UpdateError> {
        let mut value = Self::new()?;
        value.test_asset_base = Some(base);
        value.test_asset_http = Some(
            reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| UpdateError::InvalidStage)?,
        );
        Ok(value)
    }

    #[cfg(test)]
    pub(crate) fn test_asset_transport(&self) -> Option<(&str, reqwest::Client)> {
        Some((
            self.test_asset_base.as_deref()?,
            self.test_asset_http.clone()?,
        ))
    }

    /// Selects the exact newer package and checksum assets for `target`.
    ///
    /// # Errors
    /// Returns a bounded discovery, cancellation, target, or release error.
    pub async fn check(
        &self,
        cancellation: &CancellationToken,
        current_version: &str,
        target: &Target,
    ) -> Result<Candidate, UpdateError> {
        let current =
            parse_version(current_version, true).map_err(|_| UpdateError::DevelopmentBuild)?;
        package_name(target, "VERSION")?;
        let request = self
            .http
            .get(&self.endpoint)
            .header(ACCEPT, "application/vnd.github+json")
            .header(USER_AGENT, USER_AGENT_VALUE)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .build()
            .map_err(|_| UpdateError::InvalidRelease)?;
        let response = tokio::select! {
            () = cancellation.cancelled() => return Err(UpdateError::Cancelled),
            response = self.http.execute(request) => response.map_err(|_| UpdateError::InvalidRelease)?,
        };
        if response.url().as_str() != self.endpoint || response.status() != reqwest::StatusCode::OK
        {
            return Err(UpdateError::InvalidRelease);
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        loop {
            let next = tokio::select! {
                () = cancellation.cancelled() => return Err(UpdateError::Cancelled),
                next = stream.next() => next,
            };
            let Some(chunk) = next else { break };
            let chunk = chunk.map_err(|_| UpdateError::InvalidRelease)?;
            if chunk.len() > MAX_METADATA_BYTES.saturating_sub(bytes.len()) {
                return Err(UpdateError::InvalidRelease);
            }
            bytes.extend_from_slice(&chunk);
        }
        select_candidate(bytes.as_slice(), current, target)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Version([u64; 3]);

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.0[0], self.0[1], self.0[2])
    }
}

/// Parses only stable three-component versions.
///
/// # Errors
/// Rejects whitespace, prerelease/build data, leading zeros, and overflow.
pub fn parse_version(value: &str, allow_optional_v: bool) -> Result<Version, UpdateError> {
    let value = if allow_optional_v {
        value.strip_prefix('v').unwrap_or(value)
    } else {
        value.strip_prefix('v').ok_or(UpdateError::InvalidRelease)?
    };
    if value.is_empty() || value.trim() != value {
        return Err(UpdateError::InvalidRelease);
    }
    let parts = value.split('.').collect::<Vec<_>>();
    if parts.len() != 3
        || parts.iter().any(|part| {
            part.is_empty()
                || !part.bytes().all(|byte| byte.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
        })
    {
        return Err(UpdateError::InvalidRelease);
    }
    let numbers = parts
        .iter()
        .map(|part| part.parse::<u64>().map_err(|_| UpdateError::InvalidRelease))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Version([numbers[0], numbers[1], numbers[2]]))
}

/// Compares two strict stable versions.
///
/// # Errors
/// Returns an error when either input is not strict stable `X.Y.Z`.
pub fn compare_versions(left: &str, right: &str) -> Result<std::cmp::Ordering, UpdateError> {
    Ok(parse_version(left, true)?.cmp(&parse_version(right, true)?))
}

/// Returns the exact packaged release filename for one supported target.
///
/// # Errors
/// Rejects every target outside darwin/linux/windows × amd64/arm64.
pub fn package_name(target: &Target, version: &str) -> Result<String, UpdateError> {
    if !matches!(target.arch.as_str(), "amd64" | "arm64") {
        return Err(UpdateError::UnsupportedTarget);
    }
    match target.os.as_str() {
        "darwin" => Ok(format!("p-track_{version}_darwin_{}.dmg", target.arch)),
        "linux" => Ok(format!("ptrack_{version}_linux_{}.tar.gz", target.arch)),
        "windows" => Ok(format!("ptrack_{version}_windows_{}.zip", target.arch)),
        _ => Err(UpdateError::UnsupportedTarget),
    }
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    body: String,
    draft: bool,
    prerelease: bool,
    published_at: String,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: i64,
    state: String,
}

pub(crate) fn select_candidate(
    bytes: &[u8],
    current: Version,
    target: &Target,
) -> Result<Candidate, UpdateError> {
    let release: GithubRelease =
        serde_json::from_slice(bytes).map_err(|_| UpdateError::InvalidRelease)?;
    let remote = parse_version(&release.tag_name, false)?;
    if release.tag_name != format!("v{remote}") || release.draft || release.prerelease {
        return Err(UpdateError::InvalidRelease);
    }
    if remote <= current {
        return Err(UpdateError::NoUpdate);
    }
    if release.notes_too_large() {
        return Err(UpdateError::InvalidRelease);
    }
    let published = OffsetDateTime::parse(&release.published_at, &Rfc3339)
        .map_err(|_| UpdateError::InvalidRelease)?;
    if published.unix_timestamp_nanos() == -62_135_596_800_000_000_000 {
        return Err(UpdateError::InvalidRelease);
    }
    let published_at = published
        .to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .map_err(|_| UpdateError::InvalidRelease)?;
    let name = package_name(target, &remote.to_string())?;
    let package = select_asset(&release.assets, &release.tag_name, &name, MAX_ASSET_BYTES)?;
    let checksums = select_asset(
        &release.assets,
        &release.tag_name,
        "checksums.txt",
        MAX_MANIFEST_BYTES,
    )?;
    Ok(Candidate {
        version: remote.to_string(),
        tag: release.tag_name.clone(),
        page_url: format!(
            "https://github.com/ro-ag/ptrack/releases/tag/{}",
            release.tag_name
        ),
        published_at,
        notes: release.body,
        package,
        checksums,
    })
}

impl GithubRelease {
    fn notes_too_large(&self) -> bool {
        self.body.len() > MAX_NOTES_BYTES || self.published_at.is_empty()
    }
}

fn select_asset(
    assets: &[GithubAsset],
    tag: &str,
    name: &str,
    maximum: u64,
) -> Result<Asset, UpdateError> {
    let matches = assets
        .iter()
        .filter(|asset| asset.name == name)
        .collect::<Vec<_>>();
    let [asset] = matches.as_slice() else {
        return Err(UpdateError::InvalidRelease);
    };
    let size = u64::try_from(asset.size).map_err(|_| UpdateError::InvalidRelease)?;
    if asset.state != "uploaded" || size == 0 || size > maximum {
        return Err(UpdateError::InvalidRelease);
    }
    validate_asset_url(&asset.browser_download_url, tag, name)?;
    Ok(Asset {
        name: name.to_owned(),
        download_url: asset.browser_download_url.clone(),
        size_bytes: size,
    })
}

pub(crate) fn validate_asset_url(url: &str, tag: &str, name: &str) -> Result<(), UpdateError> {
    let expected = format!("https://github.com/ro-ag/ptrack/releases/download/{tag}/{name}");
    (url == expected)
        .then_some(())
        .ok_or(UpdateError::InvalidRelease)
}
