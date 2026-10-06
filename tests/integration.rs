use autoclick::config::RuleConfig;
use autoclick::matcher::{MatchRegion, MatchSet};
use autoclick::runtime::execute_match_set;
use autoclick::wayland_pointer::{ClickExecutor, ImageExtent, PlannedClick};

struct RecordingExecutor(Vec<PlannedClick>);
impl ClickExecutor for RecordingExecutor {
    fn click(&mut self, click: &PlannedClick) -> anyhow::Result<()> {
        self.0.push(click.clone());
        Ok(())
    }
}
#[test]
fn clicks_only_the_first_matching_rule_through_public_api() {
    let rules = vec![
        RuleConfig {
            target_template: "accept_button.png".to_string(),
        },
        RuleConfig {
            target_template: "ready_button.png".to_string(),
        },
    ];
    let matches = MatchSet::from([
        (
            "accept_button.png".to_string(),
            vec![MatchRegion {
                left: 10,
                top: 20,
                width: 20,
                height: 10,
            }],
        ),
        (
            "ready_button.png".to_string(),
            vec![MatchRegion {
                left: 30,
                top: 40,
                width: 20,
                height: 10,
            }],
        ),
    ]);
    let mut executor = RecordingExecutor(Vec::new());
    let planned = execute_match_set(
        &rules,
        ImageExtent {
            width: 1920,
            height: 1080,
        },
        &matches,
        None,
        &mut executor,
    )
    .unwrap();
    let click = planned.expect("the first rule matched");
    assert_eq!(executor.0, vec![click.clone()]);
    assert_eq!(
        (click.rule_index, click.output_x, click.output_y),
        (0, 20, 25)
    );
}

/// Collects everything a subscriber writes.
#[derive(Clone, Default)]
struct SharedBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for SharedBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for SharedBuffer {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

#[test]
fn log_lines_start_with_a_utc_timestamp() {
    let buffer = SharedBuffer::default();
    let subscriber = autoclick::log_format(buffer.clone())
        .with_ansi(false)
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        tracing::warn!("output DP-1 disconnected; waiting for it to return")
    });

    let logged = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    // For example `2026-10-06T12:00:00.123456Z  WARN output DP-1 ...`.
    let (timestamp, rest) = logged.split_once(' ').expect("an empty log line");
    let date = timestamp.split('T').next().unwrap();
    assert!(
        date.len() == 10 && date.as_bytes()[4] == b'-' && timestamp.ends_with('Z'),
        "no timestamp first: {logged}"
    );
    assert_eq!(
        rest.trim_start(),
        "WARN output DP-1 disconnected; waiting for it to return\n"
    );
}
