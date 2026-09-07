mod parse;

use std::io::{self, BufRead, Write};

use todo_core::{App, Snapshot};

use parse::parse;

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
    println!("todo-cli — database at {db_path}");
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
                if let Err(e) = app.dispatch(command) {
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
