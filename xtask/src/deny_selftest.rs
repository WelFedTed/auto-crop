// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask deny-selftest` (ROADMAP M0.19, M1.73).
//!
//! Runs `cargo deny` with the real `deny.toml` over tiny fixture crates outside
//! the shipping workspace: planted AGPL, GPL, non-commercial and banned-name
//! crates must each FAIL, and the clean controls must PASS. This proves the
//! policy is enforced mechanically, not by convention.
//!
//! * Licence fixtures (`agpl`, `gpl`, `ccnc`) and the controls live in
//!   `xtask/fixtures/deny-selftest/`. A directory called `control` or starting with `control-`
//!   must pass; every other one must fail. `control-m1-planned` pre-vets the M1 crates that are
//!   not in the workspace yet (`image-compare`, `turbojpeg`), pinned so a licence change upstream
//!   shows up as a red check, not a surprise.
//! * One banned-crate fixture is **generated for every entry of `[bans] deny`** in `deny.toml`
//!   (a crate of that name, MIT-licensed, as a path dependency), so a ban added later is tested
//!   without touching this file. [`REQUIRED_BANS`] lists the bans that must exist at all.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const FIXTURES: &str = "xtask/fixtures/deny-selftest";

/// Bans that must be present in `deny.toml` (M1.73): AGPL/GPL crates and the dev-only OpenCV
/// bindings. Removing one is a policy change that needs the owner (B2, B12).
pub const REQUIRED_BANS: &[&str] = &[
    "dssim-core",
    "heic",
    "jpegxl-rs",
    "x264",
    "x265",
    "purecv",
    "opencv",
];

/// A static fixture directory named `control` or `control-*` must pass; every other one must fail.
pub fn expects_failure(dir_name: &str) -> bool {
    !(dir_name == "control" || dir_name.starts_with("control-"))
}

/// Text that must appear in cargo-deny's output for a fixture to count as failing
/// for the RIGHT reason (not, say, a mistyped flag).
pub fn expected_marker(dir_name: &str) -> Option<String> {
    match dir_name {
        "agpl" => Some("AGPL-3.0".to_owned()),
        "gpl" => Some("GPL-3.0".to_owned()),
        "ccnc" => Some("CC-BY-NC".to_owned()),
        d => d
            .strip_prefix("ban-")
            .map(|name| format!("crate '{name} = 0.0.0' is explicitly banned")),
    }
}

/// The crate names in `[bans] deny` of a `deny.toml` text.
pub fn banned_crates(deny_toml: &str) -> Result<Vec<String>, String> {
    let doc: toml::Table = deny_toml.parse().map_err(|e| format!("deny.toml: {e}"))?;
    let list = doc
        .get("bans")
        .and_then(|b| b.get("deny"))
        .and_then(|d| d.as_array())
        .ok_or("deny.toml has no [bans] deny list")?;
    let mut names = Vec::new();
    for e in list {
        let name = e
            .get("crate")
            .and_then(|c| c.as_str())
            .ok_or("a [bans] deny entry has no `crate` name")?;
        names.push(name.to_owned());
    }
    Ok(names)
}

/// Required bans missing from `banned`.
pub fn missing_bans(banned: &[String]) -> Vec<&'static str> {
    REQUIRED_BANS
        .iter()
        .copied()
        .filter(|r| !banned.iter().any(|b| b == r))
        .collect()
}

fn write_ban_fixture(base: &Path, name: &str) -> Result<PathBuf, String> {
    let dir = base.join(format!("ban-{name}"));
    let dep = dir.join("dep");
    fs::create_dir_all(dep.join("src")).map_err(|e| e.to_string())?;
    fs::create_dir_all(dir.join("src")).map_err(|e| e.to_string())?;
    fs::write(
        dir.join("Cargo.toml"),
        format!(
            "[workspace]\n\n[package]\nname = \"selftest-ban-{name}\"\nversion = \"0.0.0\"\nedition = \"2024\"\nlicense = \"MIT OR Apache-2.0\"\npublish = false\n\n[dependencies]\n{name} = {{ path = \"dep\" }}\n"
        ),
    )
    .map_err(|e| e.to_string())?;
    fs::write(dir.join("src/lib.rs"), "").map_err(|e| e.to_string())?;
    fs::write(
        dep.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2024\"\nlicense = \"MIT\"\npublish = false\n"
        ),
    )
    .map_err(|e| e.to_string())?;
    fs::write(dep.join("src/lib.rs"), "").map_err(|e| e.to_string())?;
    Ok(dir.join("Cargo.toml"))
}

struct Fixture {
    name: String,
    manifest: PathBuf,
    /// Remove the generated lock file afterwards (static fixtures only; generated ones are
    /// deleted with their directory).
    static_dir: bool,
}

pub fn run(_args: &[String]) -> Result<(), String> {
    let deny_text = fs::read_to_string("deny.toml").map_err(|e| format!("deny.toml: {e}"))?;
    let banned = banned_crates(&deny_text)?;
    let missing = missing_bans(&banned);
    if !missing.is_empty() {
        return Err(format!(
            "deny.toml is missing required bans: {}",
            missing.join(", ")
        ));
    }

    let mut fixtures: Vec<Fixture> = Vec::new();
    let mut dirs: Vec<_> = fs::read_dir(FIXTURES)
        .map_err(|e| format!("cannot read {FIXTURES}: {e}"))?
        .filter_map(Result::ok)
        .filter(|d| d.path().join("Cargo.toml").exists())
        .collect();
    dirs.sort_by_key(std::fs::DirEntry::file_name);
    if dirs.is_empty() {
        return Err(format!("no fixtures found in {FIXTURES}"));
    }
    for d in &dirs {
        fixtures.push(Fixture {
            name: d.file_name().to_string_lossy().into_owned(),
            manifest: d.path().join("Cargo.toml"),
            static_dir: true,
        });
    }
    let gen_base =
        std::env::temp_dir().join(format!("auto-crop-deny-selftest-{}", std::process::id()));
    let _ = fs::remove_dir_all(&gen_base);
    for b in &banned {
        fixtures.push(Fixture {
            name: format!("ban-{b}"),
            manifest: write_ban_fixture(&gen_base, b)?,
            static_dir: false,
        });
    }

    let deny_toml = fs::canonicalize("deny.toml").map_err(|e| format!("deny.toml: {e}"))?;
    let mut problems = Vec::new();
    for f in &fixtures {
        let out = Command::new("cargo")
            .args(["deny", "--manifest-path"])
            .arg(&f.manifest)
            .arg("--config")
            .arg(&deny_toml)
            .arg("check")
            .args(["licenses", "bans", "sources"])
            .output()
            .map_err(|e| format!("cannot run cargo deny (is cargo-deny installed?): {e}"))?;
        let output_text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let failed = !out.status.success();
        let want_fail = f.name.starts_with("ban-") || expects_failure(&f.name);
        if failed
            && let Some(m) = expected_marker(&f.name)
            && !output_text.contains(&m)
        {
            problems.push(format!(
                "fixture `{}` failed, but not for the expected reason (no `{m}` in output):\n{output_text}",
                f.name
            ));
        }
        let verdict = if failed == want_fail { "ok" } else { "WRONG" };
        println!(
            "deny-selftest: {:<22} expected {:<4} got {:<4} -> {verdict}",
            f.name,
            if want_fail { "FAIL" } else { "PASS" },
            if failed { "FAIL" } else { "PASS" }
        );
        if failed != want_fail {
            problems.push(format!(
                "fixture `{}` expected {} but cargo deny {}:\n{output_text}",
                f.name,
                if want_fail { "failure" } else { "success" },
                if failed { "failed" } else { "passed" },
            ));
        }
        // Clean up generated lock files so the tree stays tidy.
        if f.static_dir
            && let Some(dir) = f.manifest.parent()
        {
            let _ = fs::remove_file(dir.join("Cargo.lock"));
        }
    }
    let _ = fs::remove_dir_all(&gen_base);
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_exist_for_every_failing_fixture() {
        for n in ["agpl", "gpl", "ccnc", "ban-heic"] {
            assert!(expected_marker(n).is_some());
        }
        assert!(expected_marker("control").is_none());
        assert!(expected_marker("control-m1-planned").is_none());
    }

    #[test]
    fn only_the_controls_are_expected_to_pass() {
        assert!(!expects_failure("control"));
        assert!(!expects_failure("control-m1-planned"));
        for n in ["agpl", "gpl", "ccnc"] {
            assert!(expects_failure(n));
        }
    }

    #[test]
    fn the_real_deny_toml_has_every_required_ban() {
        let text =
            fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../deny.toml")).unwrap();
        let banned = banned_crates(&text).unwrap();
        assert!(
            missing_bans(&banned).is_empty(),
            "{:?}",
            missing_bans(&banned)
        );
    }

    #[test]
    fn a_deny_toml_that_drops_a_ban_is_caught() {
        let text = "[bans]\ndeny = [\n { crate = \"heic\" },\n { crate = \"dssim-core\" },\n]\n";
        let banned = banned_crates(text).unwrap();
        let missing = missing_bans(&banned);
        assert!(missing.contains(&"purecv"), "{missing:?}");
        assert!(missing.contains(&"opencv"), "{missing:?}");
        assert!(!missing.contains(&"heic"));
    }

    #[test]
    fn a_ban_fixture_is_written_for_each_name() {
        let base = std::env::temp_dir().join(format!(
            "auto-crop-deny-fixture-test-{}",
            std::process::id()
        ));
        let m = write_ban_fixture(&base, "purecv").unwrap();
        let text = fs::read_to_string(&m).unwrap();
        let dep = fs::read_to_string(m.parent().unwrap().join("dep/Cargo.toml")).unwrap();
        let _ = fs::remove_dir_all(&base);
        assert!(text.contains("purecv = { path = \"dep\" }"));
        assert!(dep.contains("name = \"purecv\""));
    }
}
