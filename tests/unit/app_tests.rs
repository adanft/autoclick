fn prepared_template_mat(width: i32, height: i32) -> std::sync::Arc<Mat> {
    std::sync::Arc::new(
        Mat::new_rows_cols_with_default(height, width, CV_8UC1, Scalar::all(255.0)).unwrap(),
    )
}

struct FakePromptIo {
    responses: VecDeque<String>,
}

impl FakePromptIo {
    fn new(responses: &[&str]) -> Self {
        Self {
            responses: responses.iter().map(|value| value.to_string()).collect(),
        }
    }
}

impl PromptIo for FakePromptIo {
    fn prompt(&mut self, label: &str, default: Option<&str>) -> anyhow::Result<String> {
        let _ = (label, default);
        self.responses
            .pop_front()
            .ok_or_else(|| anyhow!("missing fake response for prompt `{label}`"))
    }
}

fn sample_monitors() -> Vec<crate::monitor::MonitorSpec> {
    vec![
        crate::monitor::MonitorSpec {
            index: 1,
            name: "DP-1".to_string(),
            width: 1920,
            height: 1080,
            origin_x: 0,
            origin_y: 0,
        },
        crate::monitor::MonitorSpec {
            index: 2,
            name: "HDMI-A-1".to_string(),
            width: 2560,
            height: 1440,
            origin_x: 1920,
            origin_y: 0,
        },
    ]
}

fn write_png(path: &std::path::Path) {
    image::RgbaImage::from_pixel(4, 3, image::Rgba([255, 0, 0, 255]))
        .save(path)
        .unwrap();
}

fn write_saved_config(
    path: &std::path::Path,
    match_threshold: f32,
    target_template: &str,
) {
    let store = crate::config::ConfigStore::from_path(path.to_path_buf());
    store
        .save(&crate::config::AppConfig {
            monitor_name: "DP-1".to_string(),
            interval_ms: 250,
            match_threshold,
            rules: vec![crate::config::RuleConfig {
                target_template: target_template.to_string(),
            }],
        })
        .unwrap();
}

fn set_path(bin_dir: &std::path::Path) -> Option<std::ffi::OsString> {
    let original_path = crate::support::capture_env("PATH");
    std::env::set_var("PATH", bin_dir);
    original_path
}

#[test]
fn startup_summary_contains_key_runtime_fields() {
    let config = crate::config::AppConfig {
        monitor_name: "DP-1".to_string(),
        interval_ms: 250,
        match_threshold: 0.95,
        rules: vec![crate::config::RuleConfig {
            target_template: "accept_button.png".to_string(),
        }],
    };
    let prepared_rules = vec![crate::matcher::PreparedRule {
        target_template: "accept_button.png".to_string(),
        template_path: "/tmp/autoclick/templates/accept_button.png".into(),
        template_size: (48, 20),
        template_mat: prepared_template_mat(48, 20),
    }];
    let monitor = crate::monitor::MonitorSpec {
        index: 1,
        name: "DP-1".to_string(),
        width: 1920,
        height: 1080,
        origin_x: 0,
        origin_y: 0,
    };

    let rendered = render_startup_summary(&config, &prepared_rules, &monitor, true);

    assert!(rendered.contains("  Runtime"));
    assert!(rendered.contains("    * Monitor:   DP-1 · 1920x1080 @ (0, 0)"));
    assert!(rendered.contains("    * Interval:  250 ms"));
    assert!(rendered.contains("    * Threshold: 0.95"));
    assert!(rendered.contains("    * Matcher:   OpenCV template matching"));
    assert!(rendered.contains("  Rules (1)"));
    assert!(rendered.contains("    1. accept_button.png"));
    assert!(rendered.contains("       - asset: /tmp/autoclick/templates/accept_button.png"));
    assert!(rendered.contains("       - size:  48x20"));
    assert!(rendered.contains("    * Ydotoold:  managed"));
    assert!(!rendered.contains("mode:"));
}

#[test]
fn startup_summary_omits_redundant_background_mode_copy() {
    let config = crate::config::AppConfig {
        monitor_name: "DP-1".to_string(),
        interval_ms: 250,
        match_threshold: 0.95,
        rules: vec![crate::config::RuleConfig {
            target_template: "accept_button.png".to_string(),
        }],
    };
    let prepared_rules = vec![crate::matcher::PreparedRule {
        target_template: "accept_button.png".to_string(),
        template_path: "/tmp/autoclick/templates/accept_button.png".into(),
        template_size: (48, 20),
        template_mat: prepared_template_mat(48, 20),
    }];
    let monitor = crate::monitor::MonitorSpec {
        index: 1,
        name: "DP-1".to_string(),
        width: 1920,
        height: 1080,
        origin_x: 0,
        origin_y: 0,
    };

    let rendered = render_startup_summary(&config, &prepared_rules, &monitor, false);

    assert!(!rendered.contains("Background"));
    assert!(!rendered.contains("background monitoring starts automatically now"));
}

#[test]
fn reuses_existing_saved_configuration_when_confirmed() {
    let dir = tempdir().unwrap();
    let store = crate::config::ConfigStore::from_path(dir.path().join("config.json"));
    let saved = crate::config::AppConfig {
        monitor_name: "HDMI-A-1".to_string(),
        interval_ms: 900,
        match_threshold: 0.88,
        rules: vec![crate::config::RuleConfig {
            target_template: "ready_button.png".to_string(),
        }],
    };
    store.save(&saved).unwrap();

    let mut io = FakePromptIo::new(&["y"]);
    let loaded = load_or_configure_with_io(&store, &sample_monitors(), &mut io).unwrap();

    assert_eq!(loaded, saved);
    assert!(io.responses.is_empty());
}

#[test]
fn reconfigures_interactively_and_lists_monitor_choices() {
    let dir = tempdir().unwrap();
    let store = crate::config::ConfigStore::from_path(dir.path().join("config.json"));
    fs::create_dir_all(store.templates_dir()).unwrap();
    write_png(&store.templates_dir().join("ready_button.png"));
    store
        .save(&crate::config::AppConfig {
            monitor_name: "DP-1".to_string(),
            interval_ms: 250,
            match_threshold: 0.95,
            rules: vec![crate::config::RuleConfig {
                target_template: "old.png".to_string(),
            }],
        })
        .unwrap();

    let mut io = FakePromptIo::new(&["n", "2", "500", "0.85", "ready_button.png", "n"]);

    let config = load_or_configure_with_io(&store, &sample_monitors(), &mut io).unwrap();

    assert_eq!(config.monitor_name, "HDMI-A-1");
    assert_eq!(config.interval_ms, 500);
    assert_eq!(config.match_threshold, 0.85);
    assert_eq!(config.rules.len(), 1);
    assert_eq!(config.rules[0].target_template, "ready_button.png");
    assert_eq!(store.load().unwrap(), config);
}

#[test]
fn reconfigures_when_saved_config_uses_legacy_schema() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    std::fs::write(
        &config_path,
        r#"{
            "monitor_name": "DP-1",
            "interval_ms": 250,
            "match_threshold": 0.95,
            "rules": [{"context": "foo", "target": "bar"}]
        }"#,
    )
    .unwrap();

    let store = crate::config::ConfigStore::from_path(config_path);
    fs::create_dir_all(store.templates_dir()).unwrap();
    write_png(&store.templates_dir().join("accept_button.png"));
    let mut io = FakePromptIo::new(&["y", "1", "250", "0.95", "accept_button.png", "n"]);

    let config = load_or_configure_with_io(&store, &sample_monitors(), &mut io).unwrap();

    assert_eq!(config.rules[0].target_template, "accept_button.png");
}

#[test]
fn reconfigures_when_saved_config_is_missing_match_threshold() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    std::fs::write(
        &config_path,
        r#"{
            "monitor_name": "DP-1",
            "interval_ms": 250,
            "rules": [{"target_template": "accept_button.png"}]
        }"#,
    )
    .unwrap();

    let store = crate::config::ConfigStore::from_path(config_path);
    fs::create_dir_all(store.templates_dir()).unwrap();
    write_png(&store.templates_dir().join("accept_button.png"));
    let mut io = FakePromptIo::new(&["y", "1", "250", "0.91", "accept_button.png", "n"]);

    let config = load_or_configure_with_io(&store, &sample_monitors(), &mut io).unwrap();

    assert_eq!(config.match_threshold, 0.91);
}

#[test]
fn reconfigures_when_saved_config_has_invalid_present_match_threshold() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    std::fs::write(
        &config_path,
        r#"{
            "monitor_name": "DP-1",
            "interval_ms": 250,
            "match_threshold": 1.5,
            "rules": [{"target_template": "accept_button.png"}]
        }"#,
    )
    .unwrap();

    let store = crate::config::ConfigStore::from_path(config_path);
    fs::create_dir_all(store.templates_dir()).unwrap();
    write_png(&store.templates_dir().join("ready_button.png"));
    let mut io = FakePromptIo::new(&["y", "2", "600", "0.82", "ready_button.png", "n"]);

    let config = load_or_configure_with_io(&store, &sample_monitors(), &mut io).unwrap();

    assert_eq!(config.monitor_name, "HDMI-A-1");
    assert_eq!(config.interval_ms, 600);
    assert_eq!(config.match_threshold, 0.82);
    assert_eq!(config.rules[0].target_template, "ready_button.png");
    assert_eq!(store.load().unwrap(), config);
}

#[test]
fn rejects_rule_target_template_with_path_segments_during_prompt() {
    let mut io = FakePromptIo::new(&["1", "250", "0.95", "../accept_button.png"]);

    let error = format!("{:#}", prompt_for_config(&sample_monitors(), &mut io).unwrap_err());

    assert!(error.contains("rule target template is invalid"));
    assert!(error.contains("must not include path segments"));
}

#[test]
fn refuses_to_save_prompted_config_when_template_asset_is_missing() {
    let dir = tempdir().unwrap();
    let store = crate::config::ConfigStore::from_path(dir.path().join("config.json"));
    fs::create_dir_all(store.templates_dir()).unwrap();
    let mut io = FakePromptIo::new(&["1", "250", "0.95", "accept_button.png", "n"]);

    let error = load_or_configure_with_io(&store, &sample_monitors(), &mut io)
        .unwrap_err()
        .to_string();

    assert!(error.contains("template asset `accept_button.png` was not found"));
    assert!(!store.exists());
}

#[test]
fn prompt_for_config_rejects_invalid_threshold_input() {
    let mut io = FakePromptIo::new(&["1", "250", "1.5"]);

    let error = prompt_for_config(&sample_monitors(), &mut io)
        .unwrap_err()
        .to_string();

    assert!(error.contains("match threshold must be between 0.0 and 1.0"));
}

#[test]
fn startup_fails_when_grim_dependency_is_missing() {
    let _guard = crate::support::lock_env();
    let dir = tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    let config_path = dir.path().join("config.json");
    let templates_dir = dir.path().join("templates");
    fs::create_dir_all(&templates_dir).unwrap();
    write_saved_config(
        &config_path,
        0.95,
        "accept_button.png",
    );
    write_png(&templates_dir.join("accept_button.png"));

    let original_path = set_path(&bin_dir);
    let original_config = crate::support::capture_env("AUTOCLICK_CONFIG_PATH");
    std::env::set_var("AUTOCLICK_CONFIG_PATH", &config_path);

    let mut io = FakePromptIo::new(&["y"]);
    let error = format!(
        "{:#}",
        run_with_io_and_monitors(&mut io, &sample_monitors()).unwrap_err()
    );

    crate::support::restore_env("PATH", original_path);
    crate::support::restore_env("AUTOCLICK_CONFIG_PATH", original_config);

    assert!(error.contains("grim dependency check failed"));
    assert!(error.contains("failed to execute grim"));
}

#[test]
fn startup_fails_when_ydotool_dependency_is_missing() {
    let _guard = crate::support::lock_env();
    let dir = tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    crate::support::write_executable_script(
        &bin_dir.join("grim"),
        "#!/bin/sh\nif [ \"$1\" = \"-h\" ]; then exit 0; fi\nexit 1\n",
    );

    let config_path = dir.path().join("config.json");
    let templates_dir = dir.path().join("templates");
    fs::create_dir_all(&templates_dir).unwrap();
    write_saved_config(
        &config_path,
        0.95,
        "accept_button.png",
    );
    write_png(&templates_dir.join("accept_button.png"));

    let original_path = set_path(&bin_dir);
    let original_config = crate::support::capture_env("AUTOCLICK_CONFIG_PATH");
    std::env::set_var("AUTOCLICK_CONFIG_PATH", &config_path);

    let mut io = FakePromptIo::new(&["y"]);
    let error = format!(
        "{:#}",
        run_with_io_and_monitors(&mut io, &sample_monitors()).unwrap_err()
    );

    crate::support::restore_env("PATH", original_path);
    crate::support::restore_env("AUTOCLICK_CONFIG_PATH", original_config);

    assert!(error.contains("ydotool dependency check failed"));
    assert!(error.contains("failed to execute ydotool"));
}

#[test]
fn startup_fails_when_template_asset_is_corrupt() {
    let _guard = crate::support::lock_env();
    let dir = tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();

    crate::support::write_executable_script(
        &bin_dir.join("grim"),
        "#!/bin/sh\nif [ \"$1\" = \"-h\" ]; then exit 0; fi\nexit 1\n",
    );

    let config_path = dir.path().join("config.json");
    let templates_dir = dir.path().join("templates");
    fs::create_dir_all(&templates_dir).unwrap();
    write_saved_config(
        &config_path,
        0.95,
        "accept_button.png",
    );
    fs::write(templates_dir.join("accept_button.png"), b"not-a-real-png").unwrap();

    let original_path = set_path(&bin_dir);
    let original_config = crate::support::capture_env("AUTOCLICK_CONFIG_PATH");
    std::env::set_var("AUTOCLICK_CONFIG_PATH", &config_path);

    let mut io = FakePromptIo::new(&["y"]);
    let error = format!(
        "{:#}",
        run_with_io_and_monitors(&mut io, &sample_monitors()).unwrap_err()
    );

    crate::support::restore_env("PATH", original_path);
    crate::support::restore_env("AUTOCLICK_CONFIG_PATH", original_config);

    assert!(error.contains("OpenCV/template validation failed"));
    assert!(error.contains("could not be read"));
}

#[test]
fn startup_reports_missing_opencv_support_as_structural_error() {
    let rules = vec![crate::config::RuleConfig {
        target_template: "accept_button.png".to_string(),
    }];

    let error = format!(
        "{:#}",
        prepare_runtime_rules_with(&rules, std::path::Path::new("/tmp/templates"), |_, _| {
            Err(anyhow!("OpenCV support is missing from this runtime"))
        })
        .unwrap_err()
    );

    assert!(error.contains("OpenCV/template validation failed"));
    assert!(error.contains("OpenCV support is missing"));
}
