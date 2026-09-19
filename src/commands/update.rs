use serde::Serialize;
use std::path::Path;

use crate::errors::CliError;
use crate::output::OutputFormat;

const REPOSITORY_URL: &str = "https://github.com/limsaehyun/suno-cli";
const INSTALL_COMMAND: &str =
    "cargo install --locked --force --git https://github.com/limsaehyun/suno-cli";

#[derive(Serialize)]
struct UpdateResult {
    current_version: &'static str,
    latest_version: String,
    status: &'static str,
    install_source: &'static str,
    update_mode: &'static str,
    upgrade_command: Option<String>,
    release_url: String,
    requires_skill_reinstall: bool,
}

#[derive(Clone, Copy)]
enum InstallSource {
    Homebrew,
    Cargo,
    Standalone,
    Unknown,
}

impl InstallSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Homebrew => "homebrew",
            Self::Cargo => "cargo",
            Self::Standalone => "standalone",
            Self::Unknown => "unknown",
        }
    }
}

fn detect_install_source() -> Result<InstallSource, CliError> {
    if let Ok(raw) = std::env::var("SUNO_INSTALL_SOURCE") {
        return match raw.trim().to_ascii_lowercase().as_str() {
            "homebrew" | "brew" => Ok(InstallSource::Homebrew),
            "cargo" => Ok(InstallSource::Cargo),
            "standalone" => Ok(InstallSource::Standalone),
            "unknown" => Ok(InstallSource::Unknown),
            other => Err(CliError::Config(format!(
                "invalid SUNO_INSTALL_SOURCE '{other}' (expected homebrew, cargo, standalone, or unknown)"
            ))),
        };
    }

    let exe = std::env::current_exe()?;
    let path = exe.to_string_lossy();
    if path.contains("/Cellar/") || path.starts_with("/opt/homebrew/bin/") {
        return Ok(InstallSource::Homebrew);
    }
    let cargo_bin = std::env::var_os("CARGO_HOME")
        .map(|p| Path::new(&p).join("bin"))
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cargo/bin")));
    if cargo_bin.is_some_and(|dir| exe.starts_with(dir)) {
        return Ok(InstallSource::Cargo);
    }
    Ok(InstallSource::Standalone)
}

pub fn run(_check: bool, _force: bool, fmt: OutputFormat, quiet: bool) -> Result<(), CliError> {
    let current = env!("CARGO_PKG_VERSION");
    let source = detect_install_source()?;
    let result = UpdateResult {
        current_version: current,
        latest_version: current.to_string(),
        status: "disabled_unsigned",
        install_source: source.as_str(),
        update_mode: "manual",
        upgrade_command: Some(INSTALL_COMMAND.to_string()),
        release_url: format!("{REPOSITORY_URL}/releases"),
        requires_skill_reinstall: true,
    };

    match fmt {
        OutputFormat::Json => crate::output::json::success(&result),
        OutputFormat::Table if !quiet => {
            eprintln!("Automatic updates are disabled until releases are signed.");
            eprintln!("Update with: {INSTALL_COMMAND}");
            eprintln!("Then run `suno skill install` to refresh the agent skill.");
        }
        OutputFormat::Table => {}
    }
    Ok(())
}
