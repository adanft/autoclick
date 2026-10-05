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
            load_template(path)
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

#[test]
fn records_the_template_color_statistics_at_startup() {
    let dir = tempdir().unwrap();
    let templates_dir = dir.path().join("templates");
    std::fs::create_dir_all(&templates_dir).unwrap();
    // Five red pixels and one blue one; OpenCV's luma is 76 for red, 29 for blue.
    write_png(&templates_dir.join("accept_button.png"), &contrasting_template(3, 2));

    let prepared = prepare_rules(
        &[crate::config::RuleConfig {
            target_template: "accept_button.png".to_string(),
        }],
        &templates_dir,
    )
    .unwrap();

    let colors = prepared[0].template_colors;
    assert_eq!(colors.mean_bgr, [255.0 / 6.0, 0.0, 255.0 * 5.0 / 6.0]);
    assert!(
        (colors.luma_std - 11045_f64.sqrt() / 6.0).abs() < 1e-9,
        "unexpected luma deviation {}",
        colors.luma_std
    );
}

#[test]
fn derives_color_statistics_from_bgr_pixels_with_opencv_luma() {
    // Red and blue luma 76 and 29, exactly as OpenCV's BGR2GRAY rounds them.
    let stats = bgr_color_stats([[0, 0, 255], [255, 0, 0]]).unwrap();

    assert_eq!(stats.mean_bgr, [127.5, 0.0, 127.5]);
    assert_eq!(stats.luma_std, 23.5);
    assert!(bgr_color_stats(std::iter::empty()).is_none());
}

#[test]
fn reads_gray_and_bgr_matrices_and_their_regions_alike() {
    use crate::support::{bgr_image, to_gray};
    use opencv::core::Rect;
    use opencv::prelude::MatTraitConst;

    let image = bgr_image(4, 3, |row, col| [(row * 4 + col) as u8 * 10, 0, 0]);
    let gray_image = bgr_image(4, 3, |row, col| [(row * 4 + col) as u8 * 10; 3]);
    let region = Rect::new(1, 1, 2, 2);

    let bgr = mat_color_stats(&image.roi(region).unwrap()).unwrap();
    let gray = mat_color_stats(&to_gray(&gray_image).roi(region).unwrap()).unwrap();

    // Pixels 5, 6, 9 and 10 (times ten) in the blue channel only.
    assert_eq!(bgr.mean_bgr, [75.0, 0.0, 0.0]);
    assert_eq!(gray.mean_bgr, [75.0; 3]);
    assert!(gray.luma_std > 0.0);
}

/// A prepared rule for a BGR `template`, as `prepare_rules` would build it.
fn color_rule(template: &Mat) -> PreparedRule {
    use opencv::prelude::MatTraitConst;

    PreparedRule {
        target_template: "button.png".to_string(),
        template_path: PathBuf::from("button.png"),
        template_size: (template.cols() as u32, template.rows() as u32),
        template_mat: Arc::new(crate::support::to_gray(template)),
        template_colors: mat_color_stats(template).unwrap(),
    }
}

/// Stamps `variant` on a colored screen and matches the exact synthetic button
/// against it, returning the region grayscale matching found and the region left
/// after the color check.
fn match_variant(variant: &Mat) -> (Vec<MatchRegion>, Vec<MatchRegion>) {
    use crate::support::{button, screen_with, to_gray, BUTTON_FILL, BUTTON_TEXT};
    use crate::support::SCREEN_BACKGROUND;
    use opencv::prelude::MatTraitConst;

    let rule = color_rule(&button(BUTTON_FILL, BUTTON_TEXT));
    let screen = screen_with(80, 40, SCREEN_BACKGROUND, variant, 30, 14);
    let rules = [rule];
    let mut matches = scan_all(&to_gray(&screen), &rules, 0.9).unwrap();
    let in_gray = matches["button.png"].clone();
    verify_colors(&mut matches, &rules, |region| {
        mat_color_stats(&screen.roi(region)?)
    })
    .unwrap();
    (in_gray, matches["button.png"].clone())
}

fn button_at_stamp() -> Vec<MatchRegion> {
    vec![MatchRegion { left: 30, top: 14, width: 24, height: 12 }]
}

#[test]
fn the_color_check_accepts_the_exact_button_and_its_hover_highlight() {
    use crate::support::{blend_toward, button, map_pixels, BUTTON_FILL, BUTTON_TEXT};

    let exact = button(BUTTON_FILL, BUTTON_TEXT);
    // The virtual pointer rests on the button after a click, lightening it.
    let hovered = map_pixels(&exact, |pixel| blend_toward(pixel, 255, 0.12));

    for variant in [&exact, &hovered] {
        assert_eq!(match_variant(variant), (button_at_stamp(), button_at_stamp()));
    }
}

#[test]
fn the_color_check_rejects_dimmed_desaturated_and_recolored_copies() {
    use crate::support::{blend_toward, button, desaturate, map_pixels, BUTTON_FILL};
    use crate::support::{BUTTON_TEXT, SAME_LUMA_RED_FILL};

    let exact = button(BUTTON_FILL, BUTTON_TEXT);
    let dimmed = map_pixels(&exact, |pixel| blend_toward(pixel, 128, 0.4));
    let gray_on_gray = map_pixels(&exact, desaturate);
    let recolored = button(SAME_LUMA_RED_FILL, BUTTON_TEXT);

    for (name, variant) in [
        ("dimmed", &dimmed),
        ("gray-on-gray", &gray_on_gray),
        ("recolored", &recolored),
    ] {
        // Grayscale matching alone cannot tell these from the real button.
        assert_eq!(
            match_variant(variant),
            (button_at_stamp(), Vec::new()),
            "{name} copy"
        );
    }
}

#[test]
fn an_inverted_button_ends_unmatched() {
    use crate::support::{button, map_pixels, BUTTON_FILL, BUTTON_TEXT};

    let inverted = map_pixels(&button(BUTTON_FILL, BUTTON_TEXT), |pixel| {
        pixel.map(|channel| 255 - channel)
    });

    assert_eq!(match_variant(&inverted).1, Vec::new());
}

#[test]
fn compares_mean_color_and_contrast_against_the_tolerances() {
    let template = ColorStats { mean_bgr: [100.0, 100.0, 100.0], luma_std: 40.0 };
    let region = |delta: f64, luma_std: f64| ColorStats {
        mean_bgr: [100.0, 100.0 - delta, 100.0],
        luma_std,
    };

    let comparison = ColorComparison::between(&template, &region(12.5, 30.0));
    assert_eq!(comparison.channel_deltas, [0.0, -12.5, 0.0]);
    assert_eq!(comparison.max_channel_delta(), 12.5);
    assert_eq!(comparison.contrast_ratio, 0.75);
    assert!(comparison.accepted());

    let at_limits = [
        (MAX_MEAN_CHANNEL_DELTA, 40.0 * MIN_CONTRAST_RATIO, true),
        (0.0, 40.0 * MAX_CONTRAST_RATIO, true),
        (MAX_MEAN_CHANNEL_DELTA + 0.5, 40.0, false),
        (0.0, 40.0 * MIN_CONTRAST_RATIO - 0.5, false),
        (0.0, 40.0 * MAX_CONTRAST_RATIO + 0.5, false),
    ];
    for (delta, luma_std, accepted) in at_limits {
        assert_eq!(
            ColorComparison::between(&template, &region(delta, luma_std)).accepted(),
            accepted,
            "delta {delta}, luma deviation {luma_std}"
        );
    }
}

#[test]
fn a_flat_template_only_accepts_an_equally_flat_region() {
    let flat = ColorStats { mean_bgr: [50.0; 3], luma_std: 0.0 };

    assert!(ColorComparison::between(&flat, &flat).accepted());
    assert!(!ColorComparison::between(&flat, &ColorStats { luma_std: 3.0, ..flat }).accepted());
}

#[test]
fn a_rejected_match_is_logged_and_its_sample_failure_is_an_error() {
    let template = ColorStats { mean_bgr: [60.0, 170.0, 40.0], luma_std: 50.0 };
    let rule = PreparedRule {
        target_template: "button.png".to_string(),
        template_path: PathBuf::from("button.png"),
        template_size: (24, 12),
        template_mat: Arc::new(Mat::default()),
        template_colors: template,
    };
    let found = || {
        MatchSet::from([(
            "button.png".to_string(),
            vec![MatchRegion { left: 3, top: 4, width: 24, height: 12 }],
        )])
    };
    let red = ColorStats { mean_bgr: [80.0, 70.0, 230.0], luma_std: 50.0 };

    let mut matches = found();
    let logs = crate::support::capture_debug_logs(|| {
        verify_colors(&mut matches, std::slice::from_ref(&rule), |region| {
            assert_eq!(region, opencv::core::Rect::new(3, 4, 24, 12));
            Ok(red)
        })
        .unwrap();
    });
    assert_eq!(matches["button.png"], Vec::new());
    assert!(logs.contains("color check rejected match"), "logs: {logs}");
    assert!(logs.contains("target_template=button.png"), "logs: {logs}");
    assert!(logs.contains("max_channel_delta=190"), "logs: {logs}");
    assert!(logs.contains("contrast_ratio=1"), "logs: {logs}");

    let mut matches = found();
    let error = verify_colors(&mut matches, &[rule], |_| Err(anyhow::anyhow!("no frame")))
        .unwrap_err();
    assert!(format!("{error:#}").contains("button.png"), "unexpected error: {error:#}");
}
