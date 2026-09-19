mod common;

use common::suno;

fn update_json(source: &str, extra: &[&str]) -> (Option<i32>, Vec<u8>, Vec<u8>) {
    let mut cmd = suno();
    cmd.env("SUNO_INSTALL_SOURCE", source)
        .arg("--json")
        .arg("update")
        .args(extra);
    let out = cmd.output().unwrap();
    (out.status.code(), out.stdout, out.stderr)
}

#[test]
fn every_install_source_disables_unsigned_self_update() {
    for source in ["homebrew", "brew", "cargo", "standalone", "unknown"] {
        let (code, stdout, stderr) = update_json(source, &[]);
        assert_eq!(
            code,
            Some(0),
            "{source}: {}",
            String::from_utf8_lossy(&stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(json["data"]["status"], "disabled_unsigned");
        assert_eq!(json["data"]["update_mode"], "manual");
        assert_eq!(
            json["data"]["upgrade_command"],
            "cargo install --locked --force --git https://github.com/limsaehyun/suno-cli"
        );
    }
}

#[test]
fn check_is_offline_and_reports_the_fork() {
    let (code, stdout, _) = update_json("standalone", &["--check"]);
    assert_eq!(code, Some(0));
    let json: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(json["data"]["status"], "disabled_unsigned");
    assert_eq!(
        json["data"]["release_url"],
        "https://github.com/limsaehyun/suno-cli/releases"
    );
}

#[test]
fn invalid_install_source_exits_2() {
    let (code, stdout, stderr) = update_json("spaceship", &["--check"]);
    assert_eq!(code, Some(2));
    assert!(stdout.is_empty());
    let json: serde_json::Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(json["error"]["code"], "config_error");
}
