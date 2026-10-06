// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Output naming and the two-phase collision plan for the N files of one scan (ROADMAP M10.21,
//! M10.22; PLAN 2.7 "What may be replaced" and 4.7).
//!
//! One grammar, one sanitiser and one collision key serve every output:
//!
//! * the template has the tokens `{name}` (the source's stem), `{n}` (the 1-based rank of the item
//!   among the included items, zero-padded to `max(2, digits(N))`) and `{ext}`; the 1-to-N default
//!   is [`DEFAULT_TEMPLATE_N`] and one included item keeps the single-item name;
//! * a name is made safe on every OS at once (no separators, drive letters, device names, trailing
//!   dots or spaces, bidi controls) whatever the template or the source name says;
//! * two names collide when their [`collision_key`]s are equal: case-folded and with the common
//!   accents folded, so `Scan_01.JPG` and `scan_01.jpg`, or an NFC and an NFD spelling of the same
//!   name, count as one file on every OS. The fold is deliberately wider than NFC plus case folding
//!   (it never separates names a file system could merge) and needs no new dependency.
//!
//! **Phase 1** ([`plan_group`]) chooses all N names at once, against the directory, the names other
//! open sources occupy and the names other groups in flight reserved. **Phase 2**
//! ([`GroupPlan::recheck`]) runs right before the commit and again inside it (the no-clobber move
//! refuses to overwrite): a name taken in between aborts with `PlanStale`, nothing is written.

use crate::error::ErrKind;
use auto_crop_codecs::Format;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The 1-to-N template (PLAN 2.7).
pub const DEFAULT_TEMPLATE_N: &str = "{name}_{n}";
/// The one-to-one template.
pub const DEFAULT_TEMPLATE_1: &str = "{name}";

/// The one rule for what may be replaced in place (PLAN 2.7 "What may be replaced"): a source is
/// replaced only if it is a single-frame image **and** this build can write its format without
/// dropping content. Every path (the single-item save, the multi-item group, the CLI) asks here;
/// nothing else decides. `None`: replaceable. `Some(notice)`: the source stays byte-identical and
/// the notice code says why (`tiff.multi_page`, `anim.first_frame_only`, `heic.multi_image`,
/// `format.write_unavailable`).
pub fn replace_refusal(format: Format, frames: u32) -> Option<&'static str> {
    if frames > 1 {
        return Some(match format {
            Format::Png | Format::Webp | Format::Gif => "anim.first_frame_only",
            Format::Heic | Format::Avif => "heic.multi_image",
            _ => "tiff.multi_page",
        });
    }
    (!format.is_encodable()).then_some("format.write_unavailable")
}

/// Longest `{name}` substituted, in characters (a source name is never trusted to be short).
const MAX_NAME_CHARS: usize = 100;
/// Longest file name produced, in bytes (255 is the common file system limit).
const MAX_FILE_BYTES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    #[error("unknown token {{{0}}}")]
    UnknownToken(String),
    #[error("an unclosed or stray brace")]
    Brace,
    #[error("the template gives an empty name")]
    Empty,
}

/// What to do when a planned name is taken (UI "Keep both" is `Rename`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnCollision {
    /// `scan_01.jpg` taken: the whole group becomes `scan (2)_01.jpg`, `scan (2)_02.jpg`, ...
    #[default]
    Rename,
    /// Write nothing and report the scan as skipped.
    Skip,
}

fn is_bidi_or_control(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
        )
}

const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Makes `s` one safe path component on every OS: illegal characters become `_`, control and bidi
/// characters are dropped, trailing dots and spaces go, a Windows device name gets a `_` prefix,
/// and an empty or dots-only result becomes `image`.
pub fn sanitise_component(s: &str) -> String {
    let mut out: String = s
        .chars()
        .filter(|c| !is_bidi_or_control(*c))
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c => c,
        })
        .collect();
    // Cap by bytes on a char boundary, leaving room for a device-name prefix.
    while out.len() > MAX_FILE_BYTES - 1 {
        out.pop();
    }
    while out.ends_with(['.', ' ']) {
        out.pop();
    }
    while out.starts_with(' ') {
        out.remove(0);
    }
    let stem_part = out.split('.').next().unwrap_or("").trim_end();
    if out.is_empty() || out.chars().all(|c| c == '.') {
        return "image".to_owned();
    }
    if RESERVED.contains(&stem_part.to_lowercase().as_str()) {
        out.insert(0, '_');
    }
    out
}

/// Diacritic table for the collision key: base letter, then the lower-case accented letters that
/// fold to it (Latin-1 Supplement and Latin Extended-A).
const FOLD: [(char, &str); 19] = [
    ('a', "àáâãäåāăą"),
    ('c', "çćĉċč"),
    ('d', "ďđ"),
    ('e', "èéêëēĕėęě"),
    ('g', "ĝğġģ"),
    ('h', "ĥħ"),
    ('i', "ìíîïĩīĭįıİ"),
    ('j', "ĵ"),
    ('k', "ķ"),
    ('l', "ĺļľŀł"),
    ('n', "ñńņň"),
    ('o', "òóôõöøōŏő"),
    ('r', "ŕŗř"),
    ('s', "śŝşšſ"),
    ('t', "ţťŧ"),
    ('u', "ùúûüũūŭůűų"),
    ('w', "ŵ"),
    ('y', "ýÿŷ"),
    ('z', "źżž"),
];

/// The key two names must differ in to be different files (M10.22): lower-cased, combining marks
/// removed, common precomposed accents folded, `ß`/`æ`/`œ` expanded, trailing dots and spaces
/// dropped. Equal keys mean "treat as the same file".
pub fn collision_key(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if is_bidi_or_control(c) {
            continue;
        }
        for lc in c.to_lowercase() {
            // Combining marks: an NFD spelling, or the dot a capital dotted I lower-cases to.
            if ('\u{0300}'..='\u{036F}').contains(&lc) {
                continue;
            }
            match lc {
                'ß' => out.push_str("ss"),
                'æ' => out.push_str("ae"),
                'œ' => out.push_str("oe"),
                // Final sigma and dotted capital I (which lower-cases to "i" plus a mark).
                'ς' => out.push('σ'),
                lc => {
                    let folded = FOLD
                        .iter()
                        .find(|(_, set)| set.contains(lc))
                        .map(|(b, _)| *b);
                    out.push(folded.unwrap_or(lc));
                }
            }
        }
    }
    while out.ends_with(['.', ' ']) {
        out.pop();
    }
    out
}

/// The key of a whole path: the key of every component, joined with `/`.
pub fn path_key(p: &Path) -> String {
    p.components()
        .map(|c| collision_key(&c.as_os_str().to_string_lossy()))
        .collect::<Vec<_>>()
        .join("/")
}

/// Names that are spoken for: other open sources, and outputs of groups still in flight.
#[derive(Debug, Clone, Default)]
pub struct ReservedKeys(HashSet<String>);

impl ReservedKeys {
    pub fn insert(&mut self, p: &Path) {
        self.0.insert(path_key(p));
    }
    pub fn contains(&self, p: &Path) -> bool {
        self.0.contains(&path_key(p))
    }
    pub fn extend_from(&mut self, other: &ReservedKeys) {
        self.0.extend(other.0.iter().cloned());
    }
    pub fn remove(&mut self, p: &Path) {
        self.0.remove(&path_key(p));
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn digits(mut n: usize) -> usize {
    let mut d = 1;
    while n >= 10 {
        n /= 10;
        d += 1;
    }
    d
}

/// Zero-padded `{n}` for output `rank` (1-based) of `total`.
pub fn pad_rank(rank: usize, total: usize) -> String {
    let w = digits(total).max(2);
    format!("{rank:0w$}")
}

/// Expands `template` into a file name (stem and extension). `rank_of` is `Some((rank, total))`
/// for a 1-to-N output. A template without `{n}` gets `_{n}` appended when there is more than one
/// output, because two outputs must never share a name. Unless the template has `{ext}`, `.ext` is
/// appended.
pub fn expand_name(
    template: &str,
    stem: &str,
    rank_of: Option<(usize, usize)>,
    ext: &str,
) -> Result<String, TemplateError> {
    if template.trim().is_empty() {
        return Err(TemplateError::Empty);
    }
    let mut tpl = template.to_owned();
    let multi = rank_of.is_some_and(|(_, total)| total > 1);
    // Parse once to learn which tokens are present.
    let mut has_n = false;
    let mut has_ext = false;
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        if rest.as_bytes()[i] == b'}' {
            return Err(TemplateError::Brace);
        }
        let Some(j) = rest[i..].find('}') else {
            return Err(TemplateError::Brace);
        };
        match &rest[i + 1..i + j] {
            "name" => {}
            "n" => has_n = true,
            "ext" => has_ext = true,
            other => return Err(TemplateError::UnknownToken(other.to_owned())),
        }
        rest = &rest[i + j + 1..];
    }
    if multi && !has_n {
        tpl.push_str("_{n}");
    }
    let name_part: String = sanitise_component(stem)
        .chars()
        .take(MAX_NAME_CHARS)
        .collect();
    let n_part = rank_of.map_or_else(String::new, |(r, t)| pad_rank(r, t));
    let ext_part = ext.trim_start_matches('.');
    let mut s = tpl
        .replace("{name}", &name_part)
        .replace("{n}", &n_part)
        .replace("{ext}", ext_part);
    if !has_ext && !ext_part.is_empty() {
        s.push('.');
        s.push_str(ext_part);
    }
    Ok(sanitise_component(&s))
}

/// Why a plan could not be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("{0}")]
    Template(#[from] TemplateError),
    /// `OnCollision::Skip` and a name is taken.
    #[error("a name is already taken")]
    Taken,
    #[error("no free name found")]
    Exhausted,
    #[error("the destination cannot be read")]
    Dir(ErrKind),
}

impl PlanError {
    pub fn kind(&self) -> ErrKind {
        match self {
            PlanError::Taken => ErrKind::PlanStale,
            PlanError::Dir(k) => *k,
            _ => ErrKind::UnsupportedOutput,
        }
    }
}

/// The names of one scan's outputs, chosen together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupPlan {
    pub dir: PathBuf,
    /// One path per output, in output (reading) order.
    pub paths: Vec<PathBuf>,
}

pub struct PlanInput<'a> {
    pub source: &'a Path,
    /// Where the outputs go (the source's folder, or `<folder>/AutoCrop`).
    pub dir: &'a Path,
    /// How many outputs (the included items).
    pub count: usize,
    /// `None` = the default for `count`.
    pub template: Option<&'a str>,
    /// Extension of every output, without the dot.
    pub ext: &'a str,
    pub on_collision: OnCollision,
    pub reserved: &'a ReservedKeys,
    /// Names this group may take over because they are its own previous, unchanged outputs
    /// (a re-save, M10.27); they count as free.
    pub own: &'a [PathBuf],
}

fn dir_keys(dir: &Path) -> Result<HashSet<String>, PlanError> {
    match fs::read_dir(dir) {
        Ok(rd) => Ok(rd
            .flatten()
            .map(|e| collision_key(&e.file_name().to_string_lossy()))
            .collect()),
        // A folder that does not exist yet (the AutoCrop subfolder) is empty.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashSet::new()),
        Err(e) => Err(PlanError::Dir(ErrKind::from_io(&e))),
    }
}

/// Phase 1: chooses every output name of the group at once.
pub fn plan_group(input: &PlanInput<'_>) -> Result<GroupPlan, PlanError> {
    let stem = input
        .source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".to_owned());
    let template = input.template.unwrap_or(if input.count > 1 {
        DEFAULT_TEMPLATE_N
    } else {
        DEFAULT_TEMPLATE_1
    });
    let existing = dir_keys(input.dir)?;
    let own: HashSet<String> = input.own.iter().map(|p| path_key(p)).collect();
    let taken = |p: &Path| {
        let key = path_key(p);
        if own.contains(&key) {
            return false;
        }
        let name_key = collision_key(&p.file_name().unwrap_or_default().to_string_lossy());
        existing.contains(&name_key) || input.reserved.contains(p)
    };
    for attempt in 1..=1000usize {
        let base = if attempt == 1 {
            stem.clone()
        } else {
            format!("{stem} ({attempt})")
        };
        let mut paths = Vec::with_capacity(input.count);
        for rank in 1..=input.count {
            let rank_of = (input.count > 1).then_some((rank, input.count));
            let name = expand_name(template, &base, rank_of, input.ext)?;
            paths.push(input.dir.join(name));
        }
        // Two outputs of the group itself must differ as well (a hostile template cannot cause it
        // because `{n}` is forced, but the check is cheap and total).
        let mut seen = HashSet::new();
        let internal_clash = !paths.iter().all(|p| seen.insert(path_key(p)));
        if internal_clash {
            return Err(PlanError::Exhausted);
        }
        if paths.iter().any(|p| taken(p)) {
            if input.on_collision == OnCollision::Skip {
                return Err(PlanError::Taken);
            }
            continue;
        }
        return Ok(GroupPlan {
            dir: input.dir.to_path_buf(),
            paths,
        });
    }
    Err(PlanError::Exhausted)
}

impl GroupPlan {
    /// Phase 2: nothing in the plan may exist now (except what the group owns) and no other group
    /// may have reserved a name. `PlanStale` if the world moved since phase 1.
    pub fn recheck(&self, reserved: &ReservedKeys, own: &[PathBuf]) -> Result<(), ErrKind> {
        let own: HashSet<String> = own.iter().map(|p| path_key(p)).collect();
        for p in &self.paths {
            if own.contains(&path_key(p)) {
                continue;
            }
            if fs::symlink_metadata(p).is_ok() || reserved.contains(p) {
                return Err(ErrKind::PlanStale);
            }
            // A case or accent variant of the name that appeared since.
            let me = collision_key(&p.file_name().unwrap_or_default().to_string_lossy());
            if let Ok(rd) = fs::read_dir(&self.dir)
                && rd
                    .flatten()
                    .any(|e| collision_key(&e.file_name().to_string_lossy()) == me)
            {
                return Err(ErrKind::PlanStale);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn input<'a>(
        source: &'a Path,
        dir: &'a Path,
        count: usize,
        reserved: &'a ReservedKeys,
    ) -> PlanInput<'a> {
        PlanInput {
            source,
            dir,
            count,
            template: None,
            ext: "jpg",
            on_collision: OnCollision::Rename,
            reserved,
            own: &[],
        }
    }

    #[test]
    fn the_default_names_are_name_n_zero_padded_and_one_item_keeps_the_name() {
        assert_eq!(
            expand_name(DEFAULT_TEMPLATE_N, "scan", Some((3, 4)), "jpg").unwrap(),
            "scan_03.jpg"
        );
        assert_eq!(
            expand_name(DEFAULT_TEMPLATE_N, "scan", Some((7, 120)), "jpg").unwrap(),
            "scan_007.jpg"
        );
        assert_eq!(
            expand_name(DEFAULT_TEMPLATE_N, "scan", Some((12, 99)), "jpg").unwrap(),
            "scan_12.jpg"
        );
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("scan.jpg");
        let none = ReservedKeys::default();
        let one = plan_group(&input(&src, dir.path(), 1, &none)).unwrap();
        assert_eq!(one.paths, [dir.path().join("scan.jpg")]);
        let three = plan_group(&input(&src, dir.path(), 3, &none)).unwrap();
        let names: Vec<_> = three
            .paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["scan_01.jpg", "scan_02.jpg", "scan_03.jpg"]);
    }

    #[test]
    fn a_template_without_n_still_gets_distinct_names_and_ext_is_a_token() {
        assert_eq!(
            expand_name("{name}-cut", "a", Some((2, 3)), "png").unwrap(),
            "a-cut_02.png"
        );
        assert_eq!(
            expand_name("{name}-{n}.{ext}", "a", Some((2, 3)), "png").unwrap(),
            "a-02.png"
        );
        assert_eq!(expand_name("{name}", "a", None, "png").unwrap(), "a.png");
        for bad in ["{date}", "{name", "x}", "{}", "{ name }"] {
            assert!(expand_name(bad, "a", Some((1, 2)), "jpg").is_err(), "{bad}");
        }
        assert_eq!(expand_name("", "a", None, "jpg"), Err(TemplateError::Empty));
    }

    #[test]
    fn hostile_names_are_made_safe() {
        for (raw, want) in [
            ("../../etc/passwd", ".._.._etc_passwd"),
            ("C:\\win\\x", "C__win_x"),
            ("CON", "_CON"),
            ("nul.txt", "_nul.txt"),
            ("a\u{202E}b\u{0007}c", "abc"),
            ("trail. . ", "trail"),
            ("", "image"),
            ("...", "image"),
            ("  lead", "lead"),
        ] {
            assert_eq!(sanitise_component(raw), want, "{raw:?}");
        }
        assert!(sanitise_component(&"é".repeat(300)).len() <= MAX_FILE_BYTES);
    }

    #[test]
    fn collision_keys_fold_case_accents_and_normalisation_forms() {
        let same = [
            ("Scan_01.JPG", "scan_01.jpg"),
            ("Caf\u{e9}_01.jpg", "cafe\u{301}_01.jpg"), // NFC versus NFD
            ("Caf\u{c9}.jpg", "cafe\u{301}.jpg"),
            ("STRASSE.jpg", "Stra\u{df}e.jpg"),
            ("a.jpg ", "a.jpg"),
            ("a.jpg.", "a.jpg"),
        ];
        for (a, b) in same {
            assert_eq!(collision_key(a), collision_key(b), "{a:?} {b:?}");
        }
        assert_ne!(collision_key("a_01.jpg"), collision_key("a_02.jpg"));
        assert_ne!(collision_key("a.jpg"), collision_key("a.png"));
    }

    #[test]
    fn a_taken_name_renames_the_whole_group_together() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("scan.jpg");
        fs::write(&src, b"s").unwrap();
        // Only the second output's name is taken, by a different-case file.
        fs::write(dir.path().join("SCAN_02.JPG"), b"x").unwrap();
        let none = ReservedKeys::default();
        let p = plan_group(&input(&src, dir.path(), 3, &none)).unwrap();
        let names: Vec<_> = p
            .paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            ["scan (2)_01.jpg", "scan (2)_02.jpg", "scan (2)_03.jpg"]
        );
        // Skip writes nothing.
        let mut skip = input(&src, dir.path(), 3, &none);
        skip.on_collision = OnCollision::Skip;
        assert_eq!(plan_group(&skip), Err(PlanError::Taken));
    }

    #[test]
    fn names_of_other_queued_sources_and_groups_are_avoided() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.jpg");
        let mut reserved = ReservedKeys::default();
        // Another source in the batch is literally called a_01.jpg and will be processed too.
        reserved.insert(&dir.path().join("a_01.jpg"));
        let p = plan_group(&input(&src, dir.path(), 2, &reserved)).unwrap();
        assert_eq!(p.paths[0], dir.path().join("a (2)_01.jpg"));
        // A second group of the same source name in flight gets another base.
        for q in &p.paths {
            reserved.insert(q);
        }
        let again = plan_group(&input(&src, dir.path(), 2, &reserved)).unwrap();
        assert_eq!(again.paths[0], dir.path().join("a (3)_01.jpg"));
    }

    #[test]
    fn phase_two_notices_a_name_that_appeared_after_the_plan() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.jpg");
        let none = ReservedKeys::default();
        let plan = plan_group(&input(&src, dir.path(), 2, &none)).unwrap();
        assert_eq!(plan.recheck(&none, &[]), Ok(()));
        // The second name is taken behind our back, in another case.
        fs::write(dir.path().join("A_02.jpg"), b"x").unwrap();
        assert_eq!(plan.recheck(&none, &[]), Err(ErrKind::PlanStale));
        // Unless the group owns it (a re-save of its own unchanged output).
        assert_eq!(plan.recheck(&none, &[plan.paths[1].clone()]), Ok(()));
        // Another group reserved a name.
        let dir2 = tempfile::tempdir().unwrap();
        let plan2 = plan_group(&input(&dir2.path().join("a.jpg"), dir2.path(), 2, &none)).unwrap();
        let mut taken = ReservedKeys::default();
        taken.insert(&plan2.paths[0]);
        assert_eq!(plan2.recheck(&taken, &[]), Err(ErrKind::PlanStale));
    }

    #[test]
    fn a_re_save_may_reuse_its_own_previous_names() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.jpg");
        let old = [dir.path().join("a_01.jpg"), dir.path().join("a_02.jpg")];
        for p in &old {
            fs::write(p, b"old").unwrap();
        }
        let none = ReservedKeys::default();
        let mut inp = input(&src, dir.path(), 3, &none);
        inp.own = &old;
        let p = plan_group(&inp).unwrap();
        assert_eq!(p.paths[0], old[0]);
        assert_eq!(p.paths[2], dir.path().join("a_03.jpg"));
    }

    #[test]
    fn the_replaceability_gate_is_one_rule() {
        assert_eq!(replace_refusal(Format::Jpeg, 1), None);
        assert_eq!(replace_refusal(Format::Png, 1), None);
        // Animated PNG, animated WebP and GIF, multi-page TIFF, multi-image HEIC: never replaced.
        assert_eq!(
            replace_refusal(Format::Png, 3),
            Some("anim.first_frame_only")
        );
        assert_eq!(
            replace_refusal(Format::Webp, 2),
            Some("anim.first_frame_only")
        );
        assert_eq!(replace_refusal(Format::Tiff, 4), Some("tiff.multi_page"));
        assert_eq!(replace_refusal(Format::Heic, 2), Some("heic.multi_image"));
        // No writer in this build.
        for f in [
            Format::Tiff,
            Format::Webp,
            Format::Heic,
            Format::Avif,
            Format::Bmp,
            Format::Gif,
        ] {
            assert_eq!(
                replace_refusal(f, 1),
                Some("format.write_unavailable"),
                "{f:?}"
            );
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10_000))]

        /// M2.27: over 10,000 generated cases, the names of one group are pairwise distinct under
        /// the collision key, however hostile the stem and the extension are, and each is one
        /// sanitised component.
        #[test]
        fn names_of_a_group_never_share_a_collision_key(
            stem in "\\PC{0,30}",
            ext in "[a-zA-Z]{0,5}",
            total in 1usize..120,
        ) {
            let mut seen = HashSet::new();
            for rank in 1..=total {
                let rank_of = (total > 1).then_some((rank, total));
                if let Ok(name) = expand_name(DEFAULT_TEMPLATE_N, &stem, rank_of, &ext) {
                    prop_assert!(seen.insert(collision_key(&name)), "{name:?} collides");
                    prop_assert_eq!(Path::new(&name).components().count(), 1);
                }
            }
        }
    }

    proptest! {
        /// M10.21: whatever the source name or template, an output is one plain file name.
        #[test]
        fn no_name_escapes_the_folder_or_is_reserved(
            stem in "\\PC{0,40}",
            tpl in "[a-z{}_ .\\-]{0,12}",
            total in 1usize..200,
            rank in 1usize..200,
        ) {
            let rank = rank.min(total);
            if let Ok(name) = expand_name(&tpl, &stem, Some((rank, total)), "jpg") {
                prop_assert!(!name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']));
                prop_assert!(!name.chars().any(|c| c.is_control()));
                prop_assert!(name != "." && name != ".." && !name.is_empty());
                prop_assert!(!name.ends_with(['.', ' ']));
                let stem_part = name.split('.').next().unwrap().to_lowercase();
                prop_assert!(!RESERVED.contains(&stem_part.as_str()), "{name}");
                prop_assert!(name.len() <= MAX_FILE_BYTES);
                prop_assert_eq!(Path::new(&name).components().count(), 1);
            }
        }

        /// M10.22: the N names of a group are pairwise distinct under the collision key and none
        /// collides with an existing file or reserved name; every plan is deterministic.
        #[test]
        fn adversarial_groups_never_collide(
            stem in "[a-zA-Z\u{e9}_]{1,10}",
            count in 2usize..40,
            taken in proptest::collection::vec((1usize..40, any::<bool>()), 0..12),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join(format!("{stem}.jpg"));
            let mut reserved = ReservedKeys::default();
            for (n, upper) in &taken {
                let name = format!("{stem}_{}.jpg", pad_rank(*n, count));
                let name = if *upper { name.to_uppercase() } else { name };
                if n % 2 == 0 {
                    fs::write(dir.path().join(&name), b"x").unwrap();
                } else {
                    reserved.insert(&dir.path().join(&name));
                }
            }
            let a = plan_group(&input(&src, dir.path(), count, &reserved)).unwrap();
            let b = plan_group(&input(&src, dir.path(), count, &reserved)).unwrap();
            prop_assert_eq!(&a, &b);
            let mut keys = HashSet::new();
            for p in &a.paths {
                prop_assert!(keys.insert(path_key(p)), "duplicate {p:?}");
                prop_assert!(!reserved.contains(p));
                prop_assert!(!p.exists());
            }
            prop_assert_eq!(a.recheck(&reserved, &[]), Ok(()));
        }
    }
}
