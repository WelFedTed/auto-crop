// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `EditState` schema migrations (ROADMAP M1.05, PLAN 2.3): every stored edit passes through
//! [`migrate`], which upgrades it one version at a time with pure functions `v(n) -> v(n+1)` and
//! refuses anything newer than this build understands with [`ErrKind::SchemaTooNew`].
//!
//! It lives in the engine rather than in `core` because `core` may depend on `serde` and
//! `thiserror` only (M1.01) and this works on `serde_json::Value`.
//!
//! Never lose data: a too-new document is reported, not truncated or half-read, and the input is
//! never modified ([`migrate_ref`] works on a borrow). A caller that fails to migrate must leave
//! the stored bytes alone.
//!
//! One historical shape needs no version bump of its own: the pre-release 0.0.1 early slice wrote
//! `{"version":1,"geometry":{...}}` (a single optional quad) under the same version number 1.
//! [`normalise_early_slice`] recognises that shape (a `geometry` key and no `items` key) and
//! rewrites it to the v1 item list before anything else runs.

use auto_crop_core::{EDIT_STATE_VERSION, EditState, ErrKind};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

/// One migration step: takes a document at version `n` and returns it at version `n + 1`
/// (including the new `version` number). Pure: no I/O, no clock, no randomness.
pub type Step = fn(Value) -> Result<Value, ErrKind>;

/// The real migration chain. `STEPS[n - 1]` upgrades version `n` to `n + 1`.
pub const STEPS: &[Step] = &[v1_to_v2, v2_to_v3];

/// `v1 -> v2` (M10.17): adds the `split` block (policy `never`, which is what a single-item state
/// written before M10 means, profile `photos`, reading order, and `nextId` above every id in
/// use so no id is ever reused). Nothing else is touched, so the migration is lossless.
pub fn v1_to_v2(mut doc: Value) -> Result<Value, ErrKind> {
    let obj = doc.as_object_mut().ok_or(ErrKind::Corrupt)?;
    if !obj.contains_key("split") {
        let max_id = obj
            .get("items")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| i.get("id").and_then(Value::as_u64))
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        // 0 = "derive it from the items": an empty state stays equal to the default one.
        let next = if max_id == 0 {
            0
        } else {
            u32::try_from(max_id.saturating_add(1)).unwrap_or(u32::MAX)
        };
        obj.insert(
            "split".to_owned(),
            json!({
                "policy": "never",
                "profile": "photos",
                "orderMode": "reading",
                "nextId": next,
            }),
        );
    }
    obj.insert("version".to_owned(), json!(2));
    Ok(doc)
}

/// `v2 -> v3` (curved pages): the schema gains the geometry variant `curved`; nothing a v2
/// document can say changes meaning, so only the version is raised and the migration is lossless.
/// The bump exists so that an older build, which has no `curved`, refuses a document that holds
/// curves (`SchemaTooNew`) instead of half-reading it.
pub fn v2_to_v3(mut doc: Value) -> Result<Value, ErrKind> {
    let obj = doc.as_object_mut().ok_or(ErrKind::Corrupt)?;
    obj.insert("version".to_owned(), json!(3));
    Ok(doc)
}

/// The schema version a document claims. A missing `version` means 1 (every field is optional,
/// as in `EditState`'s `serde(default)`).
pub fn schema_version(doc: &Value) -> Result<u32, ErrKind> {
    let obj = doc.as_object().ok_or(ErrKind::Corrupt)?;
    match obj.get("version") {
        None => Ok(1),
        Some(v) => {
            let n = v.as_u64().ok_or(ErrKind::Corrupt)?;
            // Versions start at 1; anything that does not fit a u32 is not ours.
            match u32::try_from(n) {
                Ok(0) | Err(_) => Err(ErrKind::Corrupt),
                Ok(n) => Ok(n),
            }
        }
    }
}

/// Rewrites the early slice's `{"geometry": <quad>|null}` to `{"items": [...]}`. Documents of any
/// other shape come back unchanged.
pub fn normalise_early_slice(doc: Value) -> Value {
    let Value::Object(mut obj) = doc else {
        return doc;
    };
    if obj.contains_key("geometry") && !obj.contains_key("items") {
        let geometry = obj.remove("geometry").unwrap_or(Value::Null);
        let items = match geometry {
            Value::Object(mut quad) => {
                quad.insert("type".to_owned(), json!("quad"));
                vec![json!({
                    "id": 1,
                    "include": true,
                    "geometry": Value::Object(quad),
                    "origin": { "kind": "manual" },
                })]
            }
            _ => Vec::new(),
        };
        obj.insert("items".to_owned(), Value::Array(items));
    }
    Value::Object(obj)
}

/// Runs `steps` over `doc` until it reaches `current`. Generic over the chain so tests can drive a
/// future schema; the app uses [`migrate_value`].
pub fn migrate_value_with(doc: Value, current: u32, steps: &[Step]) -> Result<Value, ErrKind> {
    let mut n = schema_version(&doc)?;
    if n > current {
        return Err(ErrKind::SchemaTooNew);
    }
    let mut doc = doc;
    while n < current {
        let step = steps.get(n as usize - 1).ok_or(ErrKind::Internal)?;
        doc = step(doc)?;
        n += 1;
        // A step must leave the document at exactly the next version.
        if schema_version(&doc)? != n {
            return Err(ErrKind::Internal);
        }
    }
    Ok(doc)
}

/// [`migrate_value_with`] over the real chain, after the early-slice normalisation.
pub fn migrate_value(doc: Value) -> Result<Value, ErrKind> {
    migrate_value_with(normalise_early_slice(doc), EDIT_STATE_VERSION, STEPS)
}

/// Upgrades a stored edit to the current [`EditState`] without consuming the input.
pub fn migrate_ref(doc: &Value) -> Result<EditState, ErrKind> {
    let doc = migrate_value(doc.clone())?;
    // The tags below only exist for v1; a document that parses at the right version but not as
    // an `EditState` is corrupt, and is reported, never defaulted.
    let state = serde_json::from_value::<EditState>(doc).map_err(|_| ErrKind::Corrupt)?;
    // A state must survive being written and read back. A float beyond `f32` (for example
    // `fineDeg: 1.2e93`) parses as infinity, which JSON writes as `null`, so the saved state would
    // not reload (found by the nightly `editstate_json` fuzzer). Such a document is corrupt.
    let again = serde_json::to_string(&state)
        .ok()
        .and_then(|t| serde_json::from_str::<EditState>(&t).ok());
    if again.as_ref() != Some(&state) {
        return Err(ErrKind::Corrupt);
    }
    Ok(state)
}

/// Upgrades a stored edit to the current [`EditState`]. A newer schema gives
/// [`ErrKind::SchemaTooNew`]; callers keep their copy of `doc` (or use [`migrate_ref`]).
pub fn migrate(doc: Value) -> Result<EditState, ErrKind> {
    migrate_ref(&doc)
}

/// Parses JSON text and migrates it. Text that is not JSON is `Corrupt`.
pub fn migrate_str(json: &str) -> Result<EditState, ErrKind> {
    let doc: Value = serde_json::from_str(json).map_err(|_| ErrKind::Corrupt)?;
    migrate(doc)
}

/// `serde(deserialize_with)` for a stored `Option<EditState>`: runs the migration chain, so a
/// manifest written by the early slice still loads and a too-new one is refused as a whole.
pub fn deserialize_optional<'de, D>(d: D) -> Result<Option<EditState>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<Value>::deserialize(d)? {
        None | Some(Value::Null) => Ok(None),
        Some(v) => migrate_ref(&v)
            .map(Some)
            .map_err(|e| serde::de::Error::custom(e.user_message_key())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auto_crop_core::{Geometry, Origin};
    use std::fs;
    use std::path::PathBuf;

    fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/editstate")
    }

    fn fixtures() -> Vec<(String, Value)> {
        let mut out: Vec<(String, Value)> = fs::read_dir(fixture_dir())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .map(|p| {
                let name = p.file_stem().unwrap().to_string_lossy().into_owned();
                (
                    name,
                    serde_json::from_slice(&fs::read(&p).unwrap()).unwrap(),
                )
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    #[test]
    fn a_float_beyond_f32_is_corrupt_not_a_state_that_cannot_reload() {
        // Regression for the nightly editstate_json crash: fineDeg 1.2e93 became +inf, written as null.
        let doc = serde_json::json!({
            "version": 1,
            "orientation": {"quarterTurns": 0, "mirror": false},
            "items": [{"id": 1, "include": true,
                "geometry": {"type": "quad",
                    "corners": [{"x":0.1,"y":0.1},{"x":0.9,"y":0.1},{"x":0.9,"y":0.9},{"x":0.1,"y":0.9}],
                    "quarterTurns": 0, "mirror": false, "fineDeg": 1.2e93},
                "enhanceOverride": null, "origin": {"kind": "manual"}, "confidence": null}],
            "margin": {"kind": "paperEdge", "margin": 0.0},
            "enhance": {"mode": "original"}
        });
        assert_eq!(migrate_ref(&doc), Err(ErrKind::Corrupt));
    }

    #[test]
    fn there_are_fixtures_for_every_v1_shape() {
        let names: Vec<String> = fixtures().into_iter().map(|f| f.0).collect();
        for want in [
            "v1_empty",
            "v1_early_slice_no_crop",
            "v1_early_slice_quad",
            "v1_multi_item",
            "v1_single_quad",
            "v2_split_three_items",
            "v2_split_manual_order",
            "v3_curved_receipt",
            "v3_curved_and_quad",
        ] {
            assert!(names.iter().any(|n| n == want), "missing fixture {want}");
        }
        assert!(
            names
                .iter()
                .all(|n| n.starts_with("v1_") || n.starts_with("v2_") || n.starts_with("v3_")),
            "{names:?}"
        );
    }

    #[test]
    fn every_fixture_migrates_and_round_trips() {
        for (name, doc) in fixtures() {
            let state = migrate_ref(&doc).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(state.version, EDIT_STATE_VERSION, "{name}");
            // Serialise and migrate again: the state is a fixed point.
            let json = serde_json::to_value(&state).unwrap();
            assert_eq!(migrate_ref(&json).unwrap(), state, "{name}");
            // Canonical current-version documents reserialise byte for byte.
            if name.starts_with("v3_") {
                assert_eq!(json, doc, "{name} is not in canonical form");
            }
            // And the text path agrees.
            assert_eq!(
                migrate_str(&serde_json::to_string(&doc).unwrap()).unwrap(),
                state,
                "{name}"
            );
        }
    }

    #[test]
    fn snapshots_of_the_migrated_fixtures() {
        for (name, doc) in fixtures() {
            let state = migrate_ref(&doc).unwrap();
            insta::assert_json_snapshot!(name, state);
        }
    }

    /// M10.17: an old single-item state migrates losslessly. Everything the v1 document said is
    /// still there, unchanged; the only additions are the version and the default `split` block.
    #[test]
    fn v1_documents_migrate_losslessly_to_v2() {
        let mut n = 0;
        for (name, doc) in fixtures() {
            if !name.starts_with("v1_") || name.contains("early_slice") || name == "v1_empty" {
                continue;
            }
            n += 1;
            let migrated = serde_json::to_value(migrate_ref(&doc).unwrap()).unwrap();
            let mut got = migrated.as_object().unwrap().clone();
            let split = got.remove("split").expect("a split block");
            got.remove("version");
            let mut want = doc.as_object().unwrap().clone();
            want.remove("version");
            assert_eq!(Value::Object(got), Value::Object(want), "{name}");
            let max = doc["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["id"].as_u64().unwrap())
                .max()
                .unwrap_or(0);
            assert_eq!(
                split,
                json!({"policy": "never", "profile": "photos", "orderMode": "reading", "nextId": max + 1}),
                "{name}"
            );
            // `Never` is the single-item behaviour of M2.16.
            assert_eq!(
                migrate_ref(&doc).unwrap().split.policy,
                auto_crop_core::SplitPolicy::Never
            );
        }
        assert!(n >= 2, "fixtures were checked");
        // A document with no version at all (every field optional) is version 1.
        let bare = migrate(json!({})).unwrap();
        assert_eq!((bare.version, bare.items.len()), (EDIT_STATE_VERSION, 0));
    }

    /// Curved pages: a v2 document migrates to v3 changing nothing but its version, a v3 document
    /// with curves loads and round-trips, and no curve is lost on the way.
    #[test]
    fn v2_documents_migrate_losslessly_to_v3_and_curves_survive() {
        let mut n = 0;
        for (name, doc) in fixtures() {
            if !name.starts_with("v2_") {
                continue;
            }
            n += 1;
            let migrated = serde_json::to_value(migrate_ref(&doc).unwrap()).unwrap();
            let mut got = migrated.as_object().unwrap().clone();
            let mut want = doc.as_object().unwrap().clone();
            assert_eq!(got.remove("version"), Some(json!(3)), "{name}");
            want.remove("version");
            assert_eq!(Value::Object(got), Value::Object(want), "{name}");
        }
        assert!(n >= 2);
        // The step on its own: only the version changes.
        let v2 = json!({"version": 2, "items": [], "split": {"nextId": 4}});
        let v3 = v2_to_v3(v2.clone()).unwrap();
        assert_eq!(
            v3,
            json!({"version": 3, "items": [], "split": {"nextId": 4}})
        );
        assert_eq!(v2_to_v3(json!(5)), Err(ErrKind::Corrupt));

        for name in ["v3_curved_receipt", "v3_curved_and_quad"] {
            let doc = fixtures().into_iter().find(|f| f.0 == name).unwrap().1;
            let s = migrate_ref(&doc).unwrap();
            assert!(s.has_curved(), "{name}");
            let c = s.items[0].geometry.curves().expect("curves");
            assert_eq!(c.validate(), Ok(()), "{name}");
            assert!(c.top.points().len() >= 3, "{name}: interior points kept");
        }
    }

    #[test]
    fn a_curved_document_with_a_bad_curve_is_corrupt_not_half_read() {
        let mut doc = fixtures()
            .into_iter()
            .find(|f| f.0 == "v3_curved_receipt")
            .unwrap()
            .1;
        // One point in a curve: the 2-point minimum is enforced at load.
        doc["items"][0]["geometry"]["top"] = json!([{"x": 0.2, "y": 0.05}]);
        assert_eq!(migrate_ref(&doc), Err(ErrKind::Corrupt));
        // More than 32 points: the cap.
        let many: Vec<Value> = (0..33)
            .map(|i| json!({"x": f64::from(i) / 40.0, "y": 0.0}))
            .collect();
        let mut doc = fixtures()
            .into_iter()
            .find(|f| f.0 == "v3_curved_receipt")
            .unwrap()
            .1;
        doc["items"][0]["geometry"]["top"] = Value::Array(many);
        assert_eq!(migrate_ref(&doc), Err(ErrKind::Corrupt));
        // A v3 document is too new for a build that stops at v2: the rule is unchanged.
        let too_new = json!({"version": EDIT_STATE_VERSION + 1, "items": []});
        assert_eq!(migrate_ref(&too_new), Err(ErrKind::SchemaTooNew));
    }

    #[test]
    fn a_migrated_id_counter_is_above_every_id_so_ids_are_never_reused() {
        let s = migrate(json!({"version":1,"items":[{"id":7},{"id":3}]})).unwrap();
        assert_eq!(s.split.next_id, 8);
        assert_eq!(s.next_item_id().0, 8);
        // Already-current documents are not rewritten.
        let v2 = json!({"version":2,"items":[{"id":9}],"split":{"nextId":40}});
        assert_eq!(migrate(v2).unwrap().next_item_id().0, 40);
    }

    #[test]
    fn the_early_slice_shape_becomes_one_manual_item() {
        let doc: Value = serde_json::from_str(
            r#"{"version":1,"geometry":{"corners":[{"x":0.1,"y":0.1},{"x":0.9,"y":0.1},{"x":0.9,"y":0.9},{"x":0.1,"y":0.9}],"quarterTurns":1,"fineDeg":2.5}}"#,
        )
        .unwrap();
        let s = migrate(doc).unwrap();
        assert_eq!(s.items.len(), 1);
        let q = s.quad().expect("a quad");
        assert_eq!((q.quarter_turns, q.fine_deg, q.mirror), (1, 2.5, false));
        assert_eq!(s.items[0].origin, Origin::Manual);
        assert!(matches!(s.items[0].geometry, Geometry::Quad(_)));
        // The old "no crop" record is an empty item list.
        let none = migrate(json!({"version":1,"geometry":null})).unwrap();
        assert!(none.items.is_empty() && none == EditState::default());
    }

    #[test]
    fn a_missing_version_is_version_one_and_junk_is_corrupt() {
        assert_eq!(migrate(json!({})).unwrap(), EditState::default());
        for bad in [
            json!(null),
            json!([]),
            json!(5),
            json!({"version": "1"}),
            json!({"version": 0}),
            json!({"version": -3}),
            json!({"version": 1.5}),
            json!({"version": 99999999999u64}),
            json!({"version": 1, "items": 4}),
            json!({"version": 1, "bogusField": true}),
        ] {
            assert_eq!(migrate_ref(&bad), Err(ErrKind::Corrupt), "{bad}");
        }
        assert_eq!(migrate_str("not json"), Err(ErrKind::Corrupt));
    }

    #[test]
    fn a_newer_schema_is_refused_and_the_input_is_never_touched() {
        let doc = json!({
            "version": EDIT_STATE_VERSION + 1,
            "items": [{"id": 1, "futureField": {"a": [1, 2, 3]}}],
            "somethingNew": true,
        });
        let before = doc.clone();
        assert_eq!(migrate_ref(&doc), Err(ErrKind::SchemaTooNew));
        assert_eq!(doc, before, "SchemaTooNew must not lose or alter data");
        assert_eq!(migrate(doc), Err(ErrKind::SchemaTooNew));
        assert_eq!(
            migrate_str(&format!(r#"{{"version":{}}}"#, u32::MAX)),
            Err(ErrKind::SchemaTooNew)
        );
    }

    // ---- A test-only v2 shim: drives the chain machinery that real v2 will use. ----

    fn shim_v1_to_v2(mut doc: Value) -> Result<Value, ErrKind> {
        let obj = doc.as_object_mut().ok_or(ErrKind::Corrupt)?;
        obj.insert("version".into(), json!(2));
        obj.insert("shimField".into(), json!("added by v1->v2"));
        Ok(doc)
    }

    fn shim_v2_to_v3(mut doc: Value) -> Result<Value, ErrKind> {
        let obj = doc.as_object_mut().ok_or(ErrKind::Corrupt)?;
        obj.insert("version".into(), json!(3));
        let renamed = obj.remove("shimField").unwrap_or(Value::Null);
        obj.insert("shimRenamed".into(), renamed);
        Ok(doc)
    }

    #[test]
    fn the_chain_applies_each_step_in_order() {
        let steps: &[Step] = &[shim_v1_to_v2, shim_v2_to_v3];
        let v1 = json!({"version": 1, "keep": 7});
        // v1 -> v2 only.
        let v2 = migrate_value_with(v1.clone(), 2, steps).unwrap();
        assert_eq!(v2["version"], 2);
        assert_eq!(v2["shimField"], "added by v1->v2");
        assert_eq!(v2["keep"], 7);
        // v1 -> v3 runs both steps.
        let v3 = migrate_value_with(v1.clone(), 3, steps).unwrap();
        assert_eq!(v3["version"], 3);
        assert_eq!(v3["shimRenamed"], "added by v1->v2");
        assert!(v3.get("shimField").is_none());
        // Already current: untouched. Newer than current: refused with the document intact.
        assert_eq!(migrate_value_with(v2.clone(), 2, steps).unwrap(), v2);
        assert_eq!(
            migrate_value_with(v3.clone(), 2, steps),
            Err(ErrKind::SchemaTooNew)
        );
        // A step that forgets to bump the version, or a missing step, is an internal error.
        let broken: &[Step] = &[|d| Ok(d)];
        assert_eq!(
            migrate_value_with(v1.clone(), 2, broken),
            Err(ErrKind::Internal)
        );
        assert_eq!(migrate_value_with(v1, 2, &[]), Err(ErrKind::Internal));
    }

    /// Found by the `editstate_json` fuzzer (M1.70, `fuzz/regressions/editstate_json/`): without
    /// serde_json's `float_roundtrip` feature a 17-digit coordinate parsed one ulp off, so what was
    /// saved did not equal what was loaded.
    #[test]
    fn coordinates_survive_a_save_and_load_exactly() {
        let doc = r#"{"items":[{"id":1,"geometry":{"type":"quad","corners":[
            {"x":0.02,"y":0.03},{"x":0.48,"y":0.02},{"x":0.47,"y":0.97},
            {"x":0.03,"y":0.98888888888888888888888888}],"quarterTurns":0,"fineDeg":0.0}}]}"#;
        let state = migrate_str(doc).unwrap();
        let saved = serde_json::to_string(&state).unwrap();
        assert_eq!(migrate_str(&saved).unwrap(), state);
        // 0.9888888888888888 is the shortest text of its f64, so it must parse back to it.
        let x = 0.9888888888888888f64;
        assert_eq!(
            serde_json::from_str::<f64>(&serde_json::to_string(&x).unwrap()).unwrap(),
            x
        );
    }

    #[test]
    fn manifests_load_through_the_chain_via_deserialize_with() {
        #[derive(Deserialize)]
        struct Holder {
            #[serde(default, deserialize_with = "deserialize_optional")]
            edit: Option<EditState>,
        }
        let early: Holder = serde_json::from_str(
            r#"{"edit":{"version":1,"geometry":{"corners":[{"x":0,"y":0},{"x":1,"y":0},{"x":1,"y":1},{"x":0,"y":1}],"quarterTurns":0,"fineDeg":0.0}}}"#,
        )
        .unwrap();
        assert!(early.edit.unwrap().quad().is_some());
        let none: Holder = serde_json::from_str(r#"{"edit":null}"#).unwrap();
        assert!(none.edit.is_none());
        let absent: Holder = serde_json::from_str("{}").unwrap();
        assert!(absent.edit.is_none());
        let too_new = serde_json::from_str::<Holder>(r#"{"edit":{"version":7}}"#);
        assert!(too_new.is_err());
    }
}
