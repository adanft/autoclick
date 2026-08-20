fn write_png(path: &std::path::Path, image: &RgbaImage) {
    image.save(path).unwrap();
}

/// Builds a template carrying real contrast, which `prepare_rules` requires.
fn contrasting_template(width: u32, height: u32) -> RgbaImage {
    let mut image = RgbaImage::from_pixel(width, height, Rgba([255, 0, 0, 255]));
    image.put_pixel(0, 0, Rgba([0, 0, 255, 255]));
    image
}

fn match_result(rows: i32, cols: i32, values: &[f32]) -> Mat {
    let mut result =
        Mat::new_rows_cols_with_default(rows, cols, CV_32FC1, Scalar::all(0.0)).unwrap();
    for top in 0..rows {
        for left in 0..cols {
            *result.at_2d_mut::<f32>(top, left).unwrap() = values[(top * cols + left) as usize];
        }
    }
    result
}

#[test]
fn resolves_template_assets_from_templates_dir() {
    let dir = tempdir().unwrap();
    let templates_dir = dir.path().join("templates");
    std::fs::create_dir_all(&templates_dir).unwrap();
    let template_path = templates_dir.join("accept_button.png");
    write_png(&template_path, &contrasting_template(3, 2));

    let prepared = prepare_rules(
        &[crate::config::RuleConfig {
            target_template: "accept_button.png".to_string(),
        }],
        &templates_dir,
    )
    .unwrap();

    assert_eq!(prepared[0].template_path, template_path);
    assert_eq!(prepared[0].template_size, (3, 2));
}

#[test]
fn fails_when_template_asset_is_missing() {
    let dir = tempdir().unwrap();
    let error = prepare_rules(
        &[crate::config::RuleConfig {
            target_template: "missing.png".to_string(),
        }],
        dir.path(),
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("template asset `missing.png` was not found"));
}

#[test]
fn fails_when_template_asset_is_corrupt() {
    let dir = tempdir().unwrap();
    let templates_dir = dir.path().join("templates");
    std::fs::create_dir_all(&templates_dir).unwrap();
    std::fs::write(templates_dir.join("accept_button.png"), b"not-a-valid-png").unwrap();

    let error = prepare_rules(
        &[crate::config::RuleConfig {
            target_template: "accept_button.png".to_string(),
        }],
        &templates_dir,
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("template asset `accept_button.png` could not be read"));
}

#[test]
fn reuses_prepared_template_assets_for_duplicate_rules() {
    let dir = tempdir().unwrap();
    let templates_dir = dir.path().join("templates");
    std::fs::create_dir_all(&templates_dir).unwrap();
    let template_path = templates_dir.join("accept_button.png");
    write_png(&template_path, &contrasting_template(3, 2));

    let load_calls = Arc::new(Mutex::new(0_usize));
    let load_calls_for_loader = Arc::clone(&load_calls);
    let prepared = prepare_rules_with_loader(
        &[
            crate::config::RuleConfig {
                target_template: "accept_button.png".to_string(),
            },
            crate::config::RuleConfig {
                target_template: "accept_button.png".to_string(),
            },
        ],
        &templates_dir,
        move |path| {
            *load_calls_for_loader.lock().unwrap() += 1;
            load_grayscale_mat(path)
        },
    )
    .unwrap();

    assert_eq!(*load_calls.lock().unwrap(), 1);
    assert!(Arc::ptr_eq(
        &prepared[0].template_mat,
        &prepared[1].template_mat
    ));
}

#[test]
fn rejects_matches_below_threshold() {
    let result = match_result(2, 2, &[0.79, 0.10, 0.60, 0.78]);

    let scan = collect_regions(&result, (3, 2), 0.80).unwrap();

    assert!(scan.regions.is_empty());
    // The rejected score is still reported: it is what tells an operator the
    // threshold is set just above what the screen produces.
    assert_eq!(scan.best_score, Some(0.79_f32 as f64));
}

#[test]
fn accepts_matches_at_threshold() {
    let result = match_result(2, 2, &[0.79, 0.80, 0.60, 0.78]);

    let scan = collect_regions(&result, (3, 2), 0.80).unwrap();

    assert_eq!(
        scan.regions,
        vec![MatchRegion {
            left: 1,
            top: 0,
            width: 3,
            height: 2,
        }]
    );
    assert_eq!(scan.best_score, Some(0.80_f32 as f64));
}

#[test]
fn scan_all_runs_opencv_matching_for_identical_template() {
    let dir = tempdir().unwrap();
    let screenshot_path = dir.path().join("screen.png");
    let template_path = dir.path().join("accept_button.png");

    let mut screenshot = RgbaImage::from_pixel(5, 4, Rgba([0, 0, 0, 255]));
    let mut template = RgbaImage::from_pixel(2, 2, Rgba([255, 255, 255, 255]));
    template.put_pixel(1, 1, Rgba([0, 255, 0, 255]));

    for y in 0..2 {
        for x in 0..2 {
            screenshot.put_pixel(1 + x, 1 + y, *template.get_pixel(x, y));
        }
    }

    write_png(&screenshot_path, &screenshot);
    write_png(&template_path, &template);

    let prepared = prepare_rules(
        &[crate::config::RuleConfig {
            target_template: "accept_button.png".to_string(),
        }],
        dir.path(),
    )
    .unwrap();

    let screenshot_mat = load_grayscale_mat(&screenshot_path).unwrap();
    let matches = scan_all(&screenshot_mat, &prepared, 1.0).unwrap();

    assert_eq!(
        matches.get("accept_button.png").unwrap().first(),
        Some(&MatchRegion {
            left: 1,
            top: 1,
            width: 2,
            height: 2,
        })
    );
}

/// Reads the `score` field out of a captured `tracing` line.
fn logged_score(logs: &str) -> f64 {
    let tail = logs
        .split("score=")
        .nth(1)
        .unwrap_or_else(|| panic!("no score was logged: {logs}"));
    tail.chars()
        .take_while(|character| !character.is_whitespace())
        .collect::<String>()
        .parse()
        .unwrap_or_else(|error| panic!("score was not a number: {error}: {logs}"))
}

#[test]
fn logs_the_best_score_of_a_template_it_rejected() {
    // The README tells operators to tune `match_threshold` from what
    // `RUST_LOG=debug` reports. That only works if a rejected template still
    // reports how close it came, which is the whole point of tuning.
    let dir = tempdir().unwrap();
    let screenshot_path = dir.path().join("screen.png");
    let template_path = dir.path().join("accept_button.png");

    let mut screenshot = RgbaImage::from_pixel(5, 4, Rgba([0, 0, 0, 255]));
    let mut template = RgbaImage::from_pixel(2, 2, Rgba([255, 255, 255, 255]));
    template.put_pixel(1, 1, Rgba([0, 255, 0, 255]));
    for y in 0..2 {
        for x in 0..2 {
            screenshot.put_pixel(1 + x, 1 + y, *template.get_pixel(x, y));
        }
    }

    write_png(&screenshot_path, &screenshot);
    write_png(&template_path, &template);

    let prepared = prepare_rules(
        &[crate::config::RuleConfig {
            target_template: "accept_button.png".to_string(),
        }],
        dir.path(),
    )
    .unwrap();
    let screenshot_mat = load_grayscale_mat(&screenshot_path).unwrap();

    let mut matches = MatchSet::new();
    let logs = crate::support::capture_debug_logs(|| {
        // No score can reach 1.5, so the exact match on screen is rejected.
        matches = scan_all(&screenshot_mat, &prepared, 1.5).unwrap();
    });

    assert_eq!(matches.get("accept_button.png"), Some(&Vec::new()));
    let score = logged_score(&logs);
    assert!(
        score > 0.99 && score <= 1.0,
        "an exact match should score ~1.0, logged {score}: {logs}"
    );
}

#[test]
fn returns_only_the_best_match_above_threshold() {
    let result = match_result(
        3,
        4,
        &[
            0.95, 0.10, 0.95, 0.05, 0.04, 0.10, 0.30, 0.10, 0.05, 0.95, 0.10, 0.05,
        ],
    );

    let matches = collect_regions(&result, (2, 2), 0.95).unwrap().regions;

    assert_eq!(
        matches,
        vec![MatchRegion {
            left: 0,
            top: 0,
            width: 2,
            height: 2,
        }]
    );
}

#[test]
fn prefers_later_higher_score_over_earlier_threshold_match() {
    let result = match_result(1, 3, &[0.95, 0.10, 0.99]);

    let matches = collect_regions(&result, (2, 2), 0.90).unwrap().regions;

    assert_eq!(
        matches,
        vec![MatchRegion {
            left: 2,
            top: 0,
            width: 2,
            height: 2,
        }]
    );
}

#[test]
fn returns_no_candidates_for_an_empty_score_matrix() {
    let scan = collect_regions(&Mat::default(), (2, 2), 0.50).unwrap();

    assert!(scan.regions.is_empty());
    // Nothing was scored, so there is no score to report.
    assert_eq!(scan.best_score, None);
}

#[test]
fn rejects_a_uniformly_bright_region_that_does_not_contain_the_template() {
    let dir = tempdir().unwrap();
    let screenshot_path = dir.path().join("screen.png");
    let template_path = dir.path().join("accept_button.png");

    // A flat bright screen area scores ~0.996 against this template under
    // TM_CCORR_NORMED because that mode never subtracts the mean. The window has
    // no variance, so a mean-subtracted mode must score it at zero instead.
    let screenshot = RgbaImage::from_pixel(12, 8, Rgba([250, 250, 250, 255]));
    let mut template = RgbaImage::from_pixel(4, 4, Rgba([250, 250, 250, 255]));
    for x in 0..4 {
        template.put_pixel(x, 3, Rgba([200, 200, 200, 255]));
    }

    write_png(&screenshot_path, &screenshot);
    write_png(&template_path, &template);

    let prepared = prepare_rules(
        &[crate::config::RuleConfig {
            target_template: "accept_button.png".to_string(),
        }],
        dir.path(),
    )
    .unwrap();

    let screenshot_mat = load_grayscale_mat(&screenshot_path).unwrap();
    let matches = scan_all(&screenshot_mat, &prepared, 0.90).unwrap();

    assert_eq!(matches.get("accept_button.png"), Some(&Vec::new()));
}

#[test]
fn rejects_a_nan_maximum_instead_of_clicking_its_location() {
    // OpenCV seeds minMaxLoc with the first element, so a NaN anywhere can become
    // the reported maximum at (0, 0). Accepting it would plan a click on the
    // top-left corner of the screen.
    let all_nan = match_result(1, 3, &[f32::NAN, f32::NAN, f32::NAN]);
    let leading_nan = match_result(1, 3, &[f32::NAN, 0.97, 0.10]);

    assert!(collect_regions(&all_nan, (2, 2), 0.90)
        .unwrap()
        .regions
        .is_empty());
    assert!(collect_regions(&leading_nan, (2, 2), 0.90)
        .unwrap()
        .regions
        .is_empty());
}

#[test]
fn refuses_a_uniform_template_that_would_match_everywhere() {
    let dir = tempdir().unwrap();
    let templates_dir = dir.path().join("templates");
    std::fs::create_dir_all(&templates_dir).unwrap();
    write_png(
        &templates_dir.join("flat.png"),
        &RgbaImage::from_pixel(4, 4, Rgba([250, 250, 250, 255])),
    );

    let error = prepare_rules(
        &[crate::config::RuleConfig {
            target_template: "flat.png".to_string(),
        }],
        &templates_dir,
    )
    .unwrap_err()
    .to_string();

    assert!(
        error.contains("is a single uniform color"),
        "unexpected error: {error}"
    );
}

#[test]
fn accepts_a_template_with_any_real_contrast() {
    let dir = tempdir().unwrap();
    let templates_dir = dir.path().join("templates");
    std::fs::create_dir_all(&templates_dir).unwrap();
    let mut template = RgbaImage::from_pixel(4, 4, Rgba([250, 250, 250, 255]));
    template.put_pixel(0, 0, Rgba([249, 249, 249, 255]));
    write_png(&templates_dir.join("subtle.png"), &template);

    let prepared = prepare_rules(
        &[crate::config::RuleConfig {
            target_template: "subtle.png".to_string(),
        }],
        &templates_dir,
    )
    .unwrap();

    assert_eq!(prepared[0].template_size, (4, 4));
}
