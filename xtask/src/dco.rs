// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask check-dco [<rev-range>]` (ROADMAP M0.17).
//!
//! Every non-merge commit in the range must carry a `Signed-off-by:` trailer
//! whose name and e-mail match the commit author (Developer Certificate of
//! Origin; no CLA). Bot authors on the allow-list are skipped. The default
//! range is `HEAD^..HEAD`.

use std::process::Command;

const BOTS: &[&str] = &["dependabot[bot]", "github-actions[bot]"];

/// True if `body` has a Signed-off-by trailer matching the author.
pub fn signed_off(author_name: &str, author_email: &str, body: &str) -> bool {
    body.lines().any(|l| {
        let Some(rest) = l.trim().strip_prefix("Signed-off-by:") else {
            return false;
        };
        let rest = rest.trim();
        let (Some(lt), Some(gt)) = (rest.rfind('<'), rest.rfind('>')) else {
            return false;
        };
        if lt >= gt {
            return false;
        }
        let name = rest[..lt].trim();
        let email = rest[lt + 1..gt].trim();
        name.eq_ignore_ascii_case(author_name.trim())
            && email.eq_ignore_ascii_case(author_email.trim())
    })
}

pub fn is_bot(author_name: &str) -> bool {
    BOTS.contains(&author_name)
}

pub fn run(args: &[String]) -> Result<(), String> {
    let range = args
        .first()
        .cloned()
        .unwrap_or_else(|| "HEAD^..HEAD".to_owned());
    let out = Command::new("git")
        .args([
            "log",
            "--no-merges",
            "--format=%H%x1f%an%x1f%ae%x1f%B%x1e",
            &range,
        ])
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git log {range} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut bad = Vec::new();
    let mut n = 0;
    for rec in text.split('\u{1e}') {
        let rec = rec.trim_matches('\n');
        let mut f = rec.splitn(4, '\u{1f}');
        let (Some(sha), Some(name), Some(email), Some(body)) =
            (f.next(), f.next(), f.next(), f.next())
        else {
            continue;
        };
        n += 1;
        if is_bot(name) || signed_off(name, email, body) {
            continue;
        }
        bad.push(format!(
            "{} by {name} <{email}> has no matching Signed-off-by (use `git commit -s`)",
            &sha[..sha.len().min(12)]
        ));
    }
    if bad.is_empty() {
        println!("check-dco: {n} commit(s) in {range} signed off");
        Ok(())
    } else {
        Err(bad.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_trailer_passes() {
        let body =
            "feat: x\n\nSigned-off-by: WelFedTed <34222508+WelFedTed@users.noreply.github.com>\n";
        assert!(signed_off(
            "WelFedTed",
            "34222508+WelFedTed@users.noreply.github.com",
            body
        ));
    }

    #[test]
    fn unsigned_fails() {
        assert!(!signed_off(
            "A",
            "a@x.org",
            "feat: x\n\nCo-Authored-By: B <b@x.org>\n"
        ));
    }

    #[test]
    fn mismatched_identity_fails() {
        assert!(!signed_off("A", "a@x.org", "Signed-off-by: B <b@x.org>"));
        assert!(!signed_off(
            "A",
            "a@x.org",
            "Signed-off-by: A <other@x.org>"
        ));
    }

    #[test]
    fn email_match_is_case_insensitive() {
        assert!(signed_off("A", "A@X.org", "Signed-off-by: A <a@x.org>"));
    }

    #[test]
    fn bots_are_skipped() {
        assert!(is_bot("dependabot[bot]"));
        assert!(!is_bot("someone"));
    }
}
