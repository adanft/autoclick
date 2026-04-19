use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Output};

/// Verifies that the `ydotool` client binary is installed and executable.
pub(crate) fn verify_ydotool_binary() -> Result<()> {
    let ydotool_status = Command::new("ydotool")
        .arg("--help")
        .output()
        .context("failed to execute ydotool")?;

    if !ydotool_status.status.success() {
        bail!("ydotool is unavailable or returned a non-zero status");
    }

    Ok(())
}

pub(crate) fn run_mousemove(socket_path: &Path, x_value: &str, y_value: &str) -> Result<()> {
    let args = ["mousemove", "--absolute", "-x", x_value, "-y", y_value];
    let output = Command::new("ydotool")
        .env("YDOTOOL_SOCKET", socket_path)
        .args(args)
        .output()
        .context("failed to execute ydotool mousemove action")?;

    if !output.status.success() {
        bail!(format_command_failure(
            "ydotool mousemove",
            &args,
            socket_path,
            &output,
        ));
    }

    Ok(())
}

/// Runs `ydotool` with the provided socket and arguments.
pub(crate) fn run_ydotool<const N: usize>(socket_path: &Path, args: [&str; N]) -> Result<()> {
    let output = Command::new("ydotool")
        .env("YDOTOOL_SOCKET", socket_path)
        .args(args)
        .output()
        .context("failed to execute ydotool action")?;

    if !output.status.success() {
        bail!(format_command_failure(
            "ydotool action",
            &args,
            socket_path,
            &output,
        ));
    }

    Ok(())
}

pub(crate) fn format_command_failure(
    label: &str,
    args: &[&str],
    socket_path: &Path,
    output: &Output,
) -> String {
    let exit = output
        .status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "terminated by signal".to_string());
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    format!(
        "{label} failed | socket={} | exit={} | args={} | stdout=`{}` | stderr=`{}`",
        socket_path.display(),
        exit,
        args.join(" "),
        stdout,
        stderr,
    )
}
