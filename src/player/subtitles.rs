use anyhow::{Context as _, Result, ensure};
use serde::Deserialize;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const API: &str = "https://api.opensubtitles.com/api/v1";
const MAX_SIZE: u64 = 8 * 1024 * 1024;

pub struct Credentials {
    pub key: String,
    pub token: String,
}

#[derive(Deserialize)]
struct SearchResponse {
    data: Vec<Entry>,
}
#[derive(Deserialize)]
struct Entry {
    attributes: Attributes,
}
#[derive(Deserialize)]
struct Attributes {
    language: String,
    files: Vec<RemoteFile>,
}
#[derive(Deserialize)]
struct RemoteFile {
    file_id: u64,
    file_name: String,
}

pub struct Match {
    pub id: u64,
    pub title: String,
}

fn curl() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("System32/curl.exe")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/usr/bin/curl")
    }
}

fn client(out: &Path) -> Command {
    let mut command = Command::new(curl());
    command
        .args([
            "--disable",
            "--silent",
            "--fail",
            "--proto",
            "=https",
            "--max-time",
            "25",
            "--max-filesize",
        ])
        .arg(MAX_SIZE.to_string())
        .arg("--user-agent")
        .arg(concat!("MyTube v", env!("CARGO_PKG_VERSION")))
        .arg("--output")
        .arg(out)
        .args(["--write-out", "%{http_code}"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}

fn complete(output: std::process::Output, path: &Path) -> Result<()> {
    let status = String::from_utf8_lossy(&output.stdout);
    ensure!(
        output.status.success() && status.starts_with('2'),
        "OpenSubtitles request failed (HTTP {status})"
    );
    let length = fs::metadata(path)?.len();
    ensure!(
        (1..=MAX_SIZE).contains(&length),
        "Empty or oversized OpenSubtitles response"
    );
    Ok(())
}

// curl reads authorization headers from stdin, keeping credentials out of the
// command line and filesystem. Only the API endpoint receives these headers.
fn api(
    route: &str,
    parameters: &[(&str, &str)],
    get: bool,
    credentials: &Credentials,
) -> Result<Vec<u8>> {
    ensure!(
        !credentials.key.is_empty()
            && [&credentials.key, &credentials.token]
                .into_iter()
                .all(|s| s.len() <= 8192 && s.bytes().all(|b| b.is_ascii_graphic())),
        "Invalid OpenSubtitles credentials"
    );
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("response");
    let mut command = client(&path);
    command.args(["--header", "@-"]);
    if get {
        command.arg("--get");
    }
    for (key, value) in parameters {
        command
            .arg("--data-urlencode")
            .arg(format!("{key}={value}"));
    }
    let mut child = command
        .arg("--url")
        .arg(format!("{API}/{route}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("Could not start curl for the subtitle request")?;
    {
        let mut headers = child.stdin.take().context("No request input")?;
        writeln!(headers, "Api-Key: {}", credentials.key)?;
        if !credentials.token.is_empty() {
            writeln!(headers, "Authorization: Bearer {}", credentials.token)?;
        }
    }
    complete(child.wait_with_output()?, &path)?;
    Ok(fs::read(path)?)
}

fn matches(json: &[u8]) -> Result<Vec<Match>> {
    let response: SearchResponse = serde_json::from_slice(json)?;
    let mut results = Vec::new();
    for entry in response.data {
        for file in entry.attributes.files {
            results.push(Match {
                id: file.file_id,
                title: format!("{} · {}", file.file_name, entry.attributes.language),
            });
        }
    }
    Ok(results)
}

pub fn search(credentials: &Credentials, title: &str, language: &str) -> Result<Vec<Match>> {
    let mut parameters = vec![("query", title)];
    if !language.trim().is_empty() {
        parameters.push(("languages", language.trim()));
    }
    matches(&api("subtitles", &parameters, true, credentials)?)
}

pub fn download(credentials: &Credentials, file_id: u64) -> Result<tempfile::TempPath> {
    #[derive(Deserialize)]
    struct Download {
        link: String,
    }
    let response: Download = serde_json::from_slice(&api(
        "download",
        &[("file_id", &file_id.to_string()), ("sub_format", "srt")],
        false,
        credentials,
    )?)?;
    ensure!(
        response.link.starts_with("https://"),
        "OpenSubtitles returned an invalid download link"
    );
    let path = tempfile::Builder::new()
        .suffix(".srt")
        .tempfile()?
        .into_temp_path();
    let output = client(&path)
        .args([
            "--location",
            "--max-redirs",
            "3",
            "--proto-redir",
            "=https",
            "--url",
        ])
        .arg(response.link)
        .stdin(Stdio::null())
        .output()?;
    complete(output, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lists_subtitle_files_by_provider_id() {
        let entries = matches(br#"{"data":[{"attributes":{"language":"fa","files":[{"file_name":"Example","file_id":17},{"file_name":"Part 2","file_id":23}]}}]}"#).unwrap();
        assert_eq!(
            entries.iter().map(|file| file.id).collect::<Vec<_>>(),
            [17, 23]
        );
        assert_eq!(entries[0].title, "Example · fa");
    }
}
