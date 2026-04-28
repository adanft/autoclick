#[test]
fn times_out_when_socket_never_appears() {
    let dir = tempdir().unwrap();
    let socket = dir.path().join("missing.sock");
    assert!(wait_for_ready_socket(&[socket], std::time::Duration::from_millis(50)).is_none());
}

#[test]
fn prefers_explicit_socket_env() {
    let _guard = crate::support::lock_env();
    let dir = tempdir().unwrap();
    let socket = dir.path().join("custom.sock");
    fs::write(&socket, b"test").unwrap();
    std::env::set_var("YDOTOOL_SOCKET", &socket);

    let candidates = socket_candidates();

    assert_eq!(candidates, vec![socket]);
    std::env::remove_var("YDOTOOL_SOCKET");
}

#[test]
fn reports_connectable_socket_as_ready() {
    let listener_dir = tempdir().unwrap();
    let socket = listener_dir.path().join("ready.sock");
    let _listener = UnixDatagram::bind(&socket).unwrap();

    assert!(socket_accepts_connections(&socket));
    assert_eq!(
        wait_for_ready_socket(&[socket.clone()], std::time::Duration::from_millis(50)),
        Some(socket)
    );
}

#[test]
fn rejects_stale_socket_file_without_listener() {
    let dir = tempdir().unwrap();
    let socket = dir.path().join("stale.sock");
    fs::write(&socket, b"stale").unwrap();

    assert!(!socket_accepts_connections(&socket));
    assert!(wait_for_ready_socket(&[socket], std::time::Duration::from_millis(50)).is_none());
}

#[test]
fn starts_and_stops_owned_ydotoold_only_when_app_manages_it() {
    let _guard = crate::support::lock_env();
    let dir = tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let socket = dir.path().join("ydotool.sock");
    let started_marker = dir.path().join("started.txt");

    crate::support::write_executable_script(&bin_dir.join("ydotool"), "#!/bin/sh\nexit 0\n");
    crate::support::write_executable_script(
        &bin_dir.join("ydotoold"),
        &format!(
            "#!/bin/sh\nexec python3 -c 'import os, signal, socket, sys, time\npath = sys.argv[1]\nmarker = sys.argv[2]\nopen(marker, \"a\").close()\ntry:\n    os.unlink(path)\nexcept FileNotFoundError:\n    pass\nsock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)\nsock.bind(path)\ndef stop(*_args):\n    sock.close()\n    try:\n        os.unlink(path)\n    except FileNotFoundError:\n        pass\n    raise SystemExit(0)\nsignal.signal(signal.SIGTERM, stop)\nsignal.signal(signal.SIGINT, stop)\nwhile True:\n    time.sleep(1)' \"{}\" \"{}\"\n",
            socket.display(),
            started_marker.display()
        ),
    );

    let original_path = crate::support::capture_env("PATH");
    let original_socket = crate::support::capture_env("YDOTOOL_SOCKET");
    let merged_path = match original_path.as_ref() {
        Some(path) => format!("{}:{}", bin_dir.display(), path.to_string_lossy()),
        None => bin_dir.display().to_string(),
    };
    std::env::set_var("PATH", merged_path);
    std::env::set_var("YDOTOOL_SOCKET", &socket);

    let mut manager = YdotoolManager::ensure_ready().unwrap();

    assert!(manager.owned());
    assert!(started_marker.exists());
    assert!(socket.exists());

    manager.shutdown().unwrap();
    thread::sleep(std::time::Duration::from_millis(100));
    crate::support::restore_env("PATH", original_path);
    crate::support::restore_env("YDOTOOL_SOCKET", original_socket);
    assert!(!manager.owned());
}

#[test]
fn leaves_external_ydotoold_running_when_socket_is_already_ready() {
    let _guard = crate::support::lock_env();
    let dir = tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let socket = dir.path().join("existing.sock");
    let started_marker = dir.path().join("should-not-start.txt");
    let _listener = UnixDatagram::bind(&socket).unwrap();

    crate::support::write_executable_script(&bin_dir.join("ydotool"), "#!/bin/sh\nexit 0\n");
    crate::support::write_executable_script(
        &bin_dir.join("ydotoold"),
        &format!(
            "#!/bin/sh\ntouch \"{}\"\nexit 0\n",
            started_marker.display()
        ),
    );

    let original_path = crate::support::capture_env("PATH");
    let original_socket = crate::support::capture_env("YDOTOOL_SOCKET");
    let merged_path = match original_path.as_ref() {
        Some(path) => format!("{}:{}", bin_dir.display(), path.to_string_lossy()),
        None => bin_dir.display().to_string(),
    };
    std::env::set_var("PATH", merged_path);
    std::env::set_var("YDOTOOL_SOCKET", &socket);

    let mut manager = YdotoolManager::ensure_ready().unwrap();
    manager.shutdown().unwrap();
    crate::support::restore_env("PATH", original_path);
    crate::support::restore_env("YDOTOOL_SOCKET", original_socket);

    assert!(!manager.owned());
    assert!(socket.exists());
    assert!(!started_marker.exists());
}

#[test]
fn execute_click_moves_with_hyprctl_and_falls_back_to_another_ready_socket_candidate() {
    let _guard = crate::support::lock_env();
    let dir = tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let stale_socket = dir.path().join("stale.sock");
    let runtime_dir = dir.path().join("runtime");
    fs::create_dir_all(&runtime_dir).unwrap();
    let active_socket = runtime_dir.join(".ydotool_socket");
    let socket_log = dir.path().join("socket.log");
    let hyprctl_log = dir.path().join("hyprctl.log");
    let _listener = UnixDatagram::bind(&active_socket).unwrap();

    crate::support::write_executable_script(
        &bin_dir.join("ydotool"),
        &format!(
            "#!/bin/sh\nprintf '%s %s\n' \"$YDOTOOL_SOCKET\" \"$*\" >> \"{}\"\nexit 0\n",
            socket_log.display()
        ),
    );
    crate::support::write_executable_script(
        &bin_dir.join("hyprctl"),
        &format!("#!/bin/sh\nprintf '%s\n' \"$*\" >> \"{}\"\nexit 0\n", hyprctl_log.display()),
    );

    let original_path = crate::support::capture_env("PATH");
    let original_socket = crate::support::capture_env("YDOTOOL_SOCKET");
    let original_runtime = crate::support::capture_env("XDG_RUNTIME_DIR");
    let merged_path = match original_path.as_ref() {
        Some(path) => format!("{}:{}", bin_dir.display(), path.to_string_lossy()),
        None => bin_dir.display().to_string(),
    };
    std::env::set_var("PATH", merged_path);
    std::env::remove_var("YDOTOOL_SOCKET");
    std::env::set_var("XDG_RUNTIME_DIR", &runtime_dir);

    let manager = YdotoolManager {
        child: None,
        owned: false,
        socket_path: stale_socket,
    };

    manager.execute_click(10, 20).unwrap();

    crate::support::restore_env("PATH", original_path);
    crate::support::restore_env("YDOTOOL_SOCKET", original_socket);
    crate::support::restore_env("XDG_RUNTIME_DIR", original_runtime);

    let logged = fs::read_to_string(&socket_log).unwrap();
    assert_eq!(
        logged.lines().collect::<Vec<_>>(),
        vec![format!("{} click 0xC0", active_socket.to_string_lossy())]
    );

    let hyprctl_logged = fs::read_to_string(&hyprctl_log).unwrap();
    assert_eq!(
        hyprctl_logged.lines().collect::<Vec<_>>(),
        vec!["dispatch movecursor 10 20"]
    );
}
