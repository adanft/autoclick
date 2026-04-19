#[test]
fn captures_full_selected_monitor_without_custom_rectangle_args() {
    let _guard = crate::support::lock_env();
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let args_path = dir.path().join("grim-args.txt");

    crate::support::write_executable_script(
        &bin_dir.join("grim"),
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--help\" ]; then exit 0; fi\nprintf '%s\\n' \"$@\" > \"{}\"\ntouch \"$3\"\n",
            args_path.display()
        ),
    );

    let original_path = crate::support::capture_env("PATH");
    let merged_path = match original_path.as_ref() {
        Some(path) => format!("{}:{}", bin_dir.display(), path.to_string_lossy()),
        None => bin_dir.display().to_string(),
    };
    std::env::set_var("PATH", merged_path);

    let capture = CaptureService::new().unwrap();
    let monitor = crate::monitor::MonitorSpec {
        index: 2,
        name: "HDMI-A-1".to_string(),
        width: 2560,
        height: 1440,
        origin_x: 1920,
        origin_y: 0,
    };

    let screenshot = capture.capture_monitor(&monitor).unwrap();
    let args = fs::read_to_string(&args_path).unwrap();
    crate::support::restore_env("PATH", original_path);

    let arg_lines: Vec<&str> = args.lines().collect();
    assert_eq!(
        arg_lines,
        vec!["-o", "HDMI-A-1", screenshot.to_string_lossy().as_ref()]
    );
}
