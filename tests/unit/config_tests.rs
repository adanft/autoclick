fn example_config() -> AppConfig {
    AppConfig {
        monitor_name: "DP-1".to_string(),
        interval_ms: 250,
        match_threshold: 0.95,
        rules: vec![RuleConfig {
            target_template: "accept_button.png".to_string(),
        }],
    }
}

#[test]
fn saves_and_loads_config() {
    let dir = tempdir().unwrap();
    let store = ConfigStore::from_path(dir.path().join("autoclick").join("config.json"));
    let config = example_config();

    store.save(&config).unwrap();
    let loaded = store.load().unwrap();

    assert_eq!(loaded, config);
}

#[test]
fn rejects_version_field() {
    let raw = r#"{
        "version": 1,
        "monitor_name": "DP-1",
        "interval_ms": 200,
        "match_threshold": 0.95,
        "rules": [{"target_template": "accept_button.png"}]
    }"#;

    let error = parse_config(raw).unwrap_err().to_string();
    assert!(error.contains("must not include a version field"));
}

#[test]
fn rejects_legacy_rule_schema() {
    let raw = r#"{
        "monitor_name": "DP-1",
        "interval_ms": 200,
        "match_threshold": 0.95,
        "rules": [{"context": "foo", "target": "bar"}]
    }"#;

    let error = parse_config(raw).unwrap_err().to_string();
    assert!(error.contains("legacy rule schema"));
}

#[test]
fn exposes_templates_dir_beside_config_file() {
    let store = ConfigStore::from_path(std::path::PathBuf::from("/tmp/autoclick/config.json"));
    assert_eq!(
        store.templates_dir(),
        std::path::PathBuf::from("/tmp/autoclick/templates")
    );
}

#[test]
fn rejects_missing_match_threshold() {
    let raw = r#"{
        "monitor_name": "DP-1",
        "interval_ms": 200,
        "rules": [{"target_template": "accept_button.png"}]
    }"#;

    let error = parse_config(raw).unwrap_err().to_string();
    assert!(error.contains("config.match_threshold is required"));
}

#[test]
fn rejects_match_threshold_out_of_range() {
    let raw = r#"{
        "monitor_name": "DP-1",
        "interval_ms": 200,
        "match_threshold": 1.5,
        "rules": [{"target_template": "accept_button.png"}]
    }"#;

    let error = parse_config(raw).unwrap_err().to_string();
    assert!(error.contains("config.match_threshold must be between 0.0 and 1.0"));
}

#[test]
fn rejects_non_finite_match_threshold_on_save() {
    let dir = tempdir().unwrap();
    let store = ConfigStore::from_path(dir.path().join("autoclick").join("config.json"));
    let mut config = example_config();
    config.match_threshold = f32::NAN;

    let error = store.save(&config).unwrap_err().to_string();
    assert!(error.contains("config.match_threshold must be finite"));
}

#[test]
fn rejects_removed_mode_field() {
    let raw = r#"{
        "mode": "background",
        "monitor_name": "DP-1",
        "interval_ms": 200,
        "match_threshold": 0.95,
        "rules": [{"target_template": "accept_button.png"}]
    }"#;

    let error = parse_config(raw).unwrap_err().to_string();
    assert!(error.contains("unsupported config field: mode"));
}

#[test]
fn rejects_rule_target_template_with_parent_traversal() {
    let raw = r#"{
        "monitor_name": "DP-1",
        "interval_ms": 200,
        "match_threshold": 0.95,
        "rules": [{"target_template": "../accept_button.png"}]
    }"#;

    let error = format!("{:#}", parse_config(raw).unwrap_err());
    assert!(error.contains("must not include path segments"));
}

#[test]
fn rejects_rule_target_template_with_absolute_path() {
    let raw = r#"{
        "monitor_name": "DP-1",
        "interval_ms": 200,
        "match_threshold": 0.95,
        "rules": [{"target_template": "/tmp/accept_button.png"}]
    }"#;

    let error = format!("{:#}", parse_config(raw).unwrap_err());
    assert!(error.contains("must be a filename inside templates/"));
}
