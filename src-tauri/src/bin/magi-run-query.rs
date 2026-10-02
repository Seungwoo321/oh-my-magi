use magi_storage::{RunEventCursor, StorageReader};
use serde::Serialize;
use std::{
    env,
    ffi::{OsStr, OsString},
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum QueryKind {
    #[default]
    Request,
    Deliberation,
    LiveDispatches,
}

#[derive(Serialize)]
struct DeliberationQuery {
    kind: &'static str,
    dossier: magi_storage::RunDossier,
    event_page: magi_storage::RunEventPage,
}

#[derive(Debug)]
struct QueryOptions {
    kind: QueryKind,
    data_root: Option<PathBuf>,
    after_sequence: u64,
    run_id: Option<String>,
    help: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let options = parse_options(env::args_os().skip(1))?;
    if options.help {
        print_help();
        return Ok(());
    }

    let run_id = options.run_id.ok_or_else(usage_error)?;
    let data_root = options
        .data_root
        .or_else(default_data_root)
        .ok_or_else(|| {
            "Set --data-root to the application's data directory on this platform.".to_owned()
        })?;
    let reader = StorageReader::open_read_only(data_root)
        .map_err(|_| "Could not open the local run store in read-only mode.".to_owned())?;
    let stdout = io::stdout();
    let mut output = stdout.lock();
    match options.kind {
        QueryKind::Request => {
            let snapshot = reader
                .get_live_run_snapshot(&run_id, options.after_sequence)
                .map_err(|_| "Could not read the requested live run snapshot.".to_owned())?;
            serde_json::to_writer(&mut output, &snapshot)
                .map_err(|_| "Could not serialize the live run snapshot.".to_owned())?;
        }
        QueryKind::Deliberation => {
            let dossier = reader
                .load_run_dossier(&run_id)
                .map_err(|_| "Could not read the requested deliberation dossier.".to_owned())?;
            let cursor =
                deliberation_cursor(dossier.replay_cursor.clone(), options.after_sequence)?;
            let event_page = reader
                .events_after_for_run(&cursor, 256)
                .map_err(|_| "Could not read the fenced deliberation events.".to_owned())?;
            serde_json::to_writer(
                &mut output,
                &DeliberationQuery {
                    kind: "deliberation",
                    dossier,
                    event_page,
                },
            )
            .map_err(|_| "Could not serialize the deliberation query.".to_owned())?;
        }
        QueryKind::LiveDispatches => {
            let projection = reader
                .load_live_dispatch_projection(&run_id)
                .map_err(|_| "Could not read the requested live dispatch projection.".to_owned())?;
            serde_json::to_writer(&mut output, &projection)
                .map_err(|_| "Could not serialize the live dispatch projection.".to_owned())?;
        }
    }
    writeln!(output).map_err(|_| "Could not write the run query.".to_owned())
}

fn deliberation_cursor(
    mut cursor: RunEventCursor,
    after_sequence: u64,
) -> Result<RunEventCursor, String> {
    if after_sequence > cursor.high_water_sequence {
        return Err(
            "--after-sequence exceeds the deliberation snapshot high-water mark.".to_owned(),
        );
    }
    cursor.after_sequence = after_sequence;
    Ok(cursor)
}

fn parse_options(arguments: impl IntoIterator<Item = OsString>) -> Result<QueryOptions, String> {
    let mut options = QueryOptions {
        kind: QueryKind::Request,
        data_root: None,
        after_sequence: 0,
        run_id: None,
        help: false,
    };
    let mut args = arguments.into_iter();
    let mut kind_seen = false;

    while let Some(argument) = args.next() {
        if argument == OsStr::new("--help") || argument == OsStr::new("-h") {
            options.help = true;
        } else if argument == OsStr::new("--kind") {
            if kind_seen {
                return Err("--kind may be supplied only once.".to_owned());
            }
            kind_seen = true;
            let value = args.next().ok_or_else(|| {
                "--kind requires request, deliberation, or live-dispatches.".to_owned()
            })?;
            options.kind = match value.to_str() {
                Some("request") => QueryKind::Request,
                Some("deliberation") => QueryKind::Deliberation,
                Some("live-dispatches") => QueryKind::LiveDispatches,
                _ => {
                    return Err(
                        "--kind must be request, deliberation, or live-dispatches.".to_owned()
                    );
                }
            };
        } else if argument == OsStr::new("--data-root") {
            let value = args
                .next()
                .ok_or_else(|| "--data-root requires a path.".to_owned())?;
            if options.data_root.replace(PathBuf::from(value)).is_some() {
                return Err("--data-root may be supplied only once.".to_owned());
            }
        } else if argument == OsStr::new("--after-sequence") {
            let value = args
                .next()
                .ok_or_else(|| "--after-sequence requires a number.".to_owned())?;
            let value = value
                .to_str()
                .ok_or_else(|| "--after-sequence must be a nonnegative integer.".to_owned())?;
            options.after_sequence = value
                .parse()
                .map_err(|_| "--after-sequence must be a nonnegative integer.".to_owned())?;
        } else if argument.to_string_lossy().starts_with('-') {
            return Err(format!("Unknown option: {}", argument.to_string_lossy()));
        } else {
            if options.run_id.is_some() {
                return Err(usage_error());
            }
            options.run_id = Some(
                argument
                    .into_string()
                    .map_err(|_| "The run ID must be valid UTF-8.".to_owned())?,
            );
        }
    }

    if options.kind == QueryKind::LiveDispatches && options.after_sequence != 0 {
        return Err("--after-sequence is not supported for live-dispatches.".to_owned());
    }
    Ok(options)
}

#[cfg(target_os = "macos")]
fn default_data_root() -> Option<PathBuf> {
    env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join("Library/Application Support")
            .join("dev.ohmymagi.console")
    })
}

#[cfg(not(target_os = "macos"))]
fn default_data_root() -> Option<PathBuf> {
    None
}

fn usage_error() -> String {
    "A run ID is required. Use --help for usage.".to_owned()
}

fn print_help() {
    println!(
        "Query persisted requests or deliberations from the app's read-only run store.\n\
         Usage: magi-run-query [--kind request|deliberation|live-dispatches] [--data-root PATH] [--after-sequence N] RUN_ID\n\
         On macOS, the default data root is ~/Library/Application Support/dev.ohmymagi.console.\n\
         --kind defaults to request and preserves the standalone request snapshot.\n\
         Deliberation returns the stored dossier and a fenced page of up to 256 events.\n\
         Live-dispatches returns the versioned reservation and frozen routing projection.\n\
         --after-sequence reads events after the given sequence; the default is 0."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<QueryOptions, String> {
        parse_options(arguments.iter().map(OsString::from))
    }

    #[test]
    fn request_default_preserves_existing_arguments() {
        let options = parse(&[
            "--data-root",
            "/tmp/query-fixture",
            "--after-sequence",
            "7",
            "run-1",
        ])
        .unwrap();
        assert_eq!(options.kind, QueryKind::Request);
        assert_eq!(options.after_sequence, 7);
        assert_eq!(options.run_id.as_deref(), Some("run-1"));
        assert_eq!(options.data_root, Some(PathBuf::from("/tmp/query-fixture")));
        assert_eq!(
            parse(&["--kind", "request", "run-1"]).unwrap().kind,
            QueryKind::Request
        );
    }

    #[test]
    fn deliberation_requires_an_explicit_valid_kind() {
        assert_eq!(
            parse(&["--kind", "deliberation", "run-1"]).unwrap().kind,
            QueryKind::Deliberation
        );
        assert!(parse(&["--kind", "automatic", "run-1"]).is_err());
        assert!(parse(&["--kind"]).is_err());
        assert!(parse(&["--kind", "request", "--kind", "deliberation", "run-1"]).is_err());
        assert!(parse(&["--after-sequence", "-1", "run-1"]).is_err());
        assert_eq!(
            parse(&["--kind", "live-dispatches", "run-1"]).unwrap().kind,
            QueryKind::LiveDispatches
        );
        assert!(
            parse(&[
                "--kind",
                "live-dispatches",
                "--after-sequence",
                "1",
                "run-1"
            ])
            .is_err()
        );
    }

    #[test]
    fn event_cursor_keeps_the_snapshot_fence() {
        let cursor = RunEventCursor {
            store_id: "store-1".to_owned(),
            store_generation: 3,
            run_id: "run-1".to_owned(),
            after_sequence: 0,
            high_water_sequence: 12,
        };
        assert!(deliberation_cursor(cursor.clone(), 13).is_err());
        let resumed = deliberation_cursor(cursor.clone(), 7).unwrap();
        assert_eq!(resumed.store_id, cursor.store_id);
        assert_eq!(resumed.store_generation, 3);
        assert_eq!(resumed.run_id, cursor.run_id);
        assert_eq!(resumed.high_water_sequence, 12);
        assert_eq!(resumed.after_sequence, 7);
        assert_eq!(deliberation_cursor(cursor, 12).unwrap().after_sequence, 12);
    }
}
