use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|text| text.trim().to_string())
}

fn git(args: &[&str]) -> Option<String> {
    git_output(args).filter(|text| !text.is_empty())
}

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    let short_commit =
        git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    let branch = git(&["branch", "--show-current"]).unwrap_or_else(|| "unknown".to_string());
    let dirty = git_output(&["status", "--porcelain"])
        .map(|s| if s.is_empty() { "false" } else { "true" })
        .unwrap_or("unknown");
    let build_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());
    let display_version = if short_commit == "unknown" {
        pkg_version.clone()
    } else if dirty == "true" {
        format!("{pkg_version}+{short_commit}.dirty")
    } else {
        format!("{pkg_version}+{short_commit}")
    };

    println!("cargo:rustc-env=MIHOMO_CLI_VERSION={display_version}");
    println!("cargo:rustc-env=MIHOMO_CLI_PKG_VERSION={pkg_version}");
    println!("cargo:rustc-env=MIHOMO_CLI_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=MIHOMO_CLI_GIT_SHORT_COMMIT={short_commit}");
    println!("cargo:rustc-env=MIHOMO_CLI_GIT_BRANCH={branch}");
    println!("cargo:rustc-env=MIHOMO_CLI_GIT_DIRTY={dirty}");
    println!("cargo:rustc-env=MIHOMO_CLI_BUILD_UNIX={build_unix}");
    println!("cargo:rustc-env=MIHOMO_CLI_BUILD_TARGET={target}");
    println!("cargo:rustc-env=MIHOMO_CLI_BUILD_PROFILE={profile}");
}
