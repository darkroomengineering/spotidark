//! Hourly update check against GitHub releases.

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

pub mod install;
#[cfg(target_os = "macos")]
mod macos;
mod transfer;
pub use transfer::{Source, download};

#[derive(Default)]
pub enum DownloadState {
    #[default]
    Idle,
    Downloading {
        received: u64,
        total: u64,
    },
    Ready(Box<install::Prepared>),
    Installing,
    Failed(String),
}

const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/darkroomengineering/spotidark/releases/latest";

/// Update-check interval.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// The version number, without a leading `v`.
    pub version: String,
    /// The release page, with every download.
    pub url: String,
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
    html_url: String,
}

/// The newest release, when it is newer than this build.
pub async fn newer_release(http: &reqwest::Client) -> Result<Option<Release>> {
    newer_release_from(http, &Source::default()).await
}

pub async fn newer_release_from(
    http: &reqwest::Client,
    source: &Source,
) -> Result<Option<Release>> {
    let response = http
        .get(source.latest())
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;
    // GitHub returns 404 until the fork publishes its first stable release.
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let latest: LatestRelease = response
        .error_for_status()?
        .json()
        .await
        .context("unexpected release listing")?;
    let version = latest.tag_name.trim_start_matches('v').to_string();
    Ok(
        is_newer(&version, env!("CARGO_PKG_VERSION")).then_some(Release {
            version,
            url: latest.html_url,
        }),
    )
}

/// `major.minor.patch`, and whether a `-rc1` or similar suffix marks it
/// as a pre-release of that version; anything else is `None`.
fn parse(version: &str) -> Option<([u64; 3], bool)> {
    let version = version.trim();
    let (numbers, pre_release) = match version.split_once('-') {
        Some((numbers, _)) => (numbers, true),
        None => (version, false),
    };
    let mut parts = numbers.split('.').map(|part| part.parse::<u64>().ok());
    let numbers = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some((numbers, pre_release))
}

/// Whether `candidate` is a newer stable version than `current`.
/// Stable releases supersede their release candidates. Other prereleases and
/// invalid versions are ignored.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some((candidate, false)), Some((current, current_pre))) => {
            candidate > current || (candidate == current && current_pre)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "demo")]
    #[tokio::test]
    async fn an_unreleased_fork_has_no_update() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source = Source::local(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let mut received = 0;
            while !request[..received]
                .windows(4)
                .any(|part| part == b"\r\n\r\n")
            {
                assert!(
                    received < request.len(),
                    "request headers exceed fixture limit"
                );
                let count = tokio::time::timeout(
                    Duration::from_secs(5),
                    stream.read(&mut request[received..]),
                )
                .await
                .unwrap()
                .unwrap();
                assert!(count > 0, "request ended before its headers");
                received += count;
            }
            assert!(request[..received].starts_with(b"GET /latest.json HTTP/1.1\r\n"));
            stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
        });
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        assert_eq!(newer_release_from(&http, &source).await.unwrap(), None);
        server.await.unwrap();
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.1.4", "0.1.3"));
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("0.1.3", "0.1.3"));
        assert!(!is_newer("0.1.2", "0.1.3"));
        assert!(
            !is_newer("0.2.0-rc1", "0.1.3"),
            "pre-releases are not announced"
        );
        assert!(!is_newer("nightly", "0.1.3"));
        // A release candidate hears about its release, and nothing older.
        assert!(is_newer("0.4.0", "0.4.0-rc1"));
        assert!(is_newer("0.4.1", "0.4.0-rc1"));
        assert!(!is_newer("0.4.0-rc1", "0.4.0"));
        assert!(!is_newer("0.4.0-rc2", "0.4.0-rc1"));
        assert!(!is_newer("0.3.0", "0.4.0-rc1"));
    }
}
