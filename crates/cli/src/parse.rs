// No clap here despite PLAN.md listing it: clap's derive parsing is built for
// argv-style flags, and even with `no_binary_name` it can't validate an index
// against the current snapshot (out-of-range checks) — that post-parse step
// is unavoidable regardless, so a plain word-split parser covers every
// required case (wrong arity, non-numeric/out-of-range index, unknown
// command) with less translation than mapping clap's `ErrorKind` variants
// back into ours would take.
use todo_core::{Command, Snapshot};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ParseError {
    #[error("usage: {command} {usage}")]
    WrongArity {
        command: &'static str,
        usage: &'static str,
    },
    #[error("not a number: {0}")]
    NonNumericIndex(String),
    #[error("no task at position {0}")]
    IndexOutOfRange(usize),
    #[error("unknown command: {0} (try: add ls done rm mv undo redo quit)")]
    UnknownCommand(String),
}

/// 1-based index into `snapshot.rows`, resolved to that task's id.
fn resolve_index(snapshot: &Snapshot, raw: &str) -> Result<String, ParseError> {
    let n: usize = raw
        .parse()
        .map_err(|_| ParseError::NonNumericIndex(raw.to_string()))?;
    snapshot
        .rows
        .get(n.wrapping_sub(1))
        .map(|row| row.id.clone())
        .ok_or(ParseError::IndexOutOfRange(n))
}

/// `0` is a sentinel meaning "the front of the list" (there is no task at
/// position 0), matching `Command::Move`'s own `after: None` convention.
fn resolve_after(snapshot: &Snapshot, raw: &str) -> Result<Option<String>, ParseError> {
    if raw == "0" {
        return Ok(None);
    }
    resolve_index(snapshot, raw).map(Some)
}

/// Parses one line of REPL input against the current snapshot (so index
/// arguments resolve to task ids). `Ok(None)` covers blank lines, `ls`
/// (rendering happens unconditionally in the caller's loop either way), and
/// `quit` (the loop detects that by string comparison before ever calling
/// this — see its own doc comment for why parse can't signal it uniquely).
pub fn parse(line: &str, snapshot: &Snapshot) -> Result<Option<Command>, ParseError> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }

    let (word, rest) = match line.split_once(char::is_whitespace) {
        Some((w, r)) => (w, r.trim()),
        None => (line, ""),
    };

    match word {
        "ls" | "quit" => Ok(None),
        "add" => {
            if rest.is_empty() {
                return Err(ParseError::WrongArity {
                    command: "add",
                    usage: "<title>",
                });
            }
            Ok(Some(Command::Add {
                title: rest.to_string(),
                after: None,
            }))
        }
        "done" => {
            if rest.is_empty() {
                return Err(ParseError::WrongArity {
                    command: "done",
                    usage: "<n>",
                });
            }
            let id = resolve_index(snapshot, rest)?;
            Ok(Some(Command::SetDone { id, done: true }))
        }
        "rm" => {
            if rest.is_empty() {
                return Err(ParseError::WrongArity {
                    command: "rm",
                    usage: "<n>",
                });
            }
            let id = resolve_index(snapshot, rest)?;
            Ok(Some(Command::Delete { id }))
        }
        "mv" => {
            let mut args = rest.split_whitespace();
            let (Some(n), Some(m)) = (args.next(), args.next()) else {
                return Err(ParseError::WrongArity {
                    command: "mv",
                    usage: "<n> <m>",
                });
            };
            let id = resolve_index(snapshot, n)?;
            let after = resolve_after(snapshot, m)?;
            Ok(Some(Command::Move { id, after }))
        }
        "undo" => Ok(Some(Command::Undo)),
        "redo" => Ok(Some(Command::Redo)),
        other => Err(ParseError::UnknownCommand(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use todo_core::{Doc, FixedClock, SeqIdSource, ViewFilter};

    fn snapshot_with_tasks(titles: &[&str]) -> Snapshot {
        let mut doc = Doc::new(1).unwrap();
        let clock = FixedClock(0);
        let ids = SeqIdSource::new();
        for title in titles {
            doc.apply(
                Command::Add {
                    title: title.to_string(),
                    after: None,
                },
                &clock,
                &ids,
            )
            .unwrap();
        }
        doc.read(ViewFilter::All, &clock)
    }

    fn empty_snapshot() -> Snapshot {
        snapshot_with_tasks(&[])
    }

    #[test]
    fn blank_line_is_none() {
        assert_eq!(parse("", &empty_snapshot()), Ok(None));
        assert_eq!(parse("   ", &empty_snapshot()), Ok(None));
    }

    #[test]
    fn quit_is_none() {
        assert_eq!(parse("quit", &empty_snapshot()), Ok(None));
    }

    #[test]
    fn ls_is_none() {
        assert_eq!(parse("ls", &empty_snapshot()), Ok(None));
    }

    #[test]
    fn add_produces_add_command() {
        let cmd = parse("add Buy milk", &empty_snapshot()).unwrap();
        assert_eq!(
            cmd,
            Some(Command::Add {
                title: "Buy milk".to_string(),
                after: None,
            })
        );
    }

    #[test]
    fn add_with_no_title_is_wrong_arity() {
        assert_eq!(
            parse("add", &empty_snapshot()),
            Err(ParseError::WrongArity {
                command: "add",
                usage: "<title>",
            })
        );
        assert_eq!(
            parse("add   ", &empty_snapshot()),
            Err(ParseError::WrongArity {
                command: "add",
                usage: "<title>",
            })
        );
    }

    #[test]
    fn done_resolves_index_to_id() {
        let snap = snapshot_with_tasks(&["a", "b"]);
        // Add(after:None) is LIFO, so row 1 is "b", row 2 is "a".
        let id_b = snap.rows[0].id.clone();
        assert_eq!(
            parse("done 1", &snap).unwrap(),
            Some(Command::SetDone {
                id: id_b,
                done: true
            })
        );
    }

    #[test]
    fn done_with_no_index_is_wrong_arity() {
        assert_eq!(
            parse("done", &empty_snapshot()),
            Err(ParseError::WrongArity {
                command: "done",
                usage: "<n>",
            })
        );
    }

    #[test]
    fn done_with_non_numeric_index_errors() {
        assert_eq!(
            parse("done abc", &empty_snapshot()),
            Err(ParseError::NonNumericIndex("abc".to_string()))
        );
    }

    #[test]
    fn done_with_out_of_range_index_errors() {
        let snap = snapshot_with_tasks(&["a"]);
        assert_eq!(parse("done 5", &snap), Err(ParseError::IndexOutOfRange(5)));
        assert_eq!(parse("done 0", &snap), Err(ParseError::IndexOutOfRange(0)));
    }

    #[test]
    fn rm_resolves_index_to_delete_command() {
        let snap = snapshot_with_tasks(&["a"]);
        let id = snap.rows[0].id.clone();
        assert_eq!(parse("rm 1", &snap).unwrap(), Some(Command::Delete { id }));
    }

    #[test]
    fn rm_with_no_index_is_wrong_arity() {
        assert_eq!(
            parse("rm", &empty_snapshot()),
            Err(ParseError::WrongArity {
                command: "rm",
                usage: "<n>",
            })
        );
    }

    #[test]
    fn mv_resolves_both_indices() {
        let snap = snapshot_with_tasks(&["a", "b", "c"]);
        let id_c = snap.rows[0].id.clone();
        let id_a = snap.rows[2].id.clone();
        assert_eq!(
            parse("mv 1 3", &snap).unwrap(),
            Some(Command::Move {
                id: id_c,
                after: Some(id_a),
            })
        );
    }

    #[test]
    fn mv_to_zero_moves_to_front() {
        let snap = snapshot_with_tasks(&["a", "b"]);
        let id_a = snap.rows[1].id.clone();
        assert_eq!(
            parse("mv 2 0", &snap).unwrap(),
            Some(Command::Move {
                id: id_a,
                after: None,
            })
        );
    }

    #[test]
    fn mv_with_missing_args_is_wrong_arity() {
        let err = ParseError::WrongArity {
            command: "mv",
            usage: "<n> <m>",
        };
        assert_eq!(parse("mv", &empty_snapshot()), Err(err.clone()));
        assert_eq!(parse("mv 1", &empty_snapshot()), Err(err));
    }

    #[test]
    fn mv_with_non_numeric_second_index_errors() {
        let snap = snapshot_with_tasks(&["a"]);
        assert_eq!(
            parse("mv 1 abc", &snap),
            Err(ParseError::NonNumericIndex("abc".to_string()))
        );
    }

    #[test]
    fn undo_and_redo() {
        assert_eq!(
            parse("undo", &empty_snapshot()).unwrap(),
            Some(Command::Undo)
        );
        assert_eq!(
            parse("redo", &empty_snapshot()).unwrap(),
            Some(Command::Redo)
        );
    }

    #[test]
    fn unknown_command_errors() {
        assert_eq!(
            parse("frobnicate", &empty_snapshot()),
            Err(ParseError::UnknownCommand("frobnicate".to_string()))
        );
    }
}
