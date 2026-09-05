//! CLI-face e2e: the actual binary, D1 envelope on stdout, v0 exit
//! codes. Env overrides point every path at a scratch dir — on the
//! phone nobody sets them.

use std::process::Command;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aginx-pkg"))
}

#[test]
fn list_json_envelope_and_usage_exit_codes() {
    let tmp = testkit::tmp("aginx-pkg-cli");
    std::fs::create_dir_all(tmp.join("bin")).unwrap();
    std::fs::write(tmp.join("bin/tool"), b"BIN").unwrap();
    let out = cli()
        .env("AGINX_PKG_BINDIR", tmp.join("bin"))
        .env("AGINX_PKG_SKILLS", tmp.join("skills"))
        .env("AGINX_PKG_UNITS", tmp.join("units"))
        .env("AGINX_PKG_STAMPS", tmp.join("stamps"))
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], serde_json::json!(true));
    assert_eq!(v["data"][0]["name"], "tool");
    assert_eq!(v["data"][0]["skill"], serde_json::json!(false));
    assert_eq!(v["meta"]["count"], serde_json::json!(1));

    // v0 exit codes: unknown subcommand / missing args = 2
    assert_eq!(cli().arg("nope").output().unwrap().status.code(), Some(2));
    assert_eq!(cli().arg("install").output().unwrap().status.code(), Some(2));
    assert_eq!(cli().arg("--help").output().unwrap().status.code(), Some(0));

    // sha mismatch on a direct install: exit 1, human line on stderr
    let src = tmp.join("tool2.bin");
    std::fs::write(&src, b"whatever").unwrap();
    let out = cli()
        .env("AGINX_PKG_BINDIR", tmp.join("bin"))
        .env("AGINX_PKG_STAMPS", tmp.join("stamps"))
        .args(["install", "tool2", src.to_str().unwrap(), "deadbeef"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("sha256 mismatch"));

    std::fs::remove_dir_all(&tmp).ok();
}
