mod parse;

use std::io::{self, BufRead, Write};

use remember_core::{App, Command, Snapshot};

use parse::parse;

/// `parse` never detects a due date itself (it has no access to `App`'s
/// clock/offset) — it always returns `due: None` for `add`. This enriches
/// that with `App::detect_due` so the same "buy milk tomorrow" phrasing the
/// GUI's quick-add will support is exercisable here too, since there's no
/// Swift toolchain in this container to try the real UI.
fn enrich_add(app: &App, command: Command) -> Command {
    let Command::Add {
        title,
        after,
        due: None,
        list_id,
    } = command
    else {
        return command;
    };
    match app.detect_due(&title) {
        Some(detection) => Command::Add {
            title: detection.stripped_title,
            after,
            due: Some(detection.due),
            list_id,
        },
        None => Command::Add {
            title,
            after,
            due: None,
            list_id,
        },
    }
}

/// Folder and file still carry the app's original name, "todo": renaming
/// them would orphan existing data. Same names as the Swift side's
/// `defaultDatabasePath`.
fn default_db_path() -> String {
    let dir = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("no.bendik.todo");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("todo.sqlite3").to_string_lossy().into_owned()
}

fn render(snapshot: &Snapshot) {
    for (i, row) in snapshot.rows.iter().enumerate() {
        let mark = if row.done { "x" } else { " " };
        let due = row.due_label.as_deref().unwrap_or("");
        println!("{:>3}. [{mark}] {} {due}", i + 1, row.title);
    }
    println!("{} active", snapshot.active_count);
}

fn main() {
    let db_path = std::env::args().nth(1).unwrap_or_else(default_db_path);
    let app = match App::open(&db_path) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("failed to open {db_path}: {e}");
            std::process::exit(1);
        }
    };
    println!("remember-cli — database at {db_path}");
    render(&app.current());

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim() == "quit" {
            break;
        }

        match parse(&line, &app.current()) {
            Ok(Some(command)) => {
                if let Err(e) = app.dispatch(enrich_add(&app, command)) {
                    println!("error: {e}");
                }
            }
            Ok(None) => {}
            Err(e) => println!("error: {e}"),
        }
        render(&app.current());
        print!("> ");
        let _ = stdout.flush();
    }

    if let Err(e) = app.flush() {
        eprintln!("failed to save: {e}");
    }
}
