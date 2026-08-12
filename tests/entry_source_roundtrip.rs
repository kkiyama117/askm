//! `EntrySource` is persisted into `state.json` and read back, so its `Serialize`
//! output must be a shape its `Deserialize` accepts. The two impls are written
//! separately, which makes that easy to break.

use askm::model::EntrySource;

fn round_trip(original: EntrySource) {
    let encoded = serde_json::to_string(&original).expect("serialize");
    let decoded: EntrySource =
        serde_json::from_str(&encoded).unwrap_or_else(|e| panic!("decode {encoded}: {e}"));

    assert_eq!(original, decoded, "via {encoded}");
}

#[test]
fn local_source_survives_a_json_round_trip() {
    round_trip(EntrySource::Local {
        path: "./plugins/agent-sdk-dev".into(),
    });
}

#[test]
fn fully_populated_git_source_survives_a_json_round_trip() {
    round_trip(EntrySource::Git {
        url: "https://github.com/x/y.git".into(),
        reference: Some("v1.5.5".into()),
        sha: Some("abc123".into()),
        subpath: Some("plugins/foo".into()),
    });
}

#[test]
fn git_source_without_optional_fields_survives_a_json_round_trip() {
    round_trip(EntrySource::Git {
        url: "https://github.com/x/y.git".into(),
        reference: None,
        sha: None,
        subpath: None,
    });
}

#[test]
fn serialized_form_matches_the_marketplace_wire_dialect() {
    let encoded = serde_json::to_value(EntrySource::Local { path: "./".into() }).unwrap();

    assert_eq!(
        encoded,
        serde_json::json!({"source": "local", "path": "./"})
    );
}
