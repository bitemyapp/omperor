use std::io::{self, BufRead, Write};

use anyhow::Result;

use crate::session::Session;

const WIDTH: usize = 72;

pub fn run(session: &mut Session) -> Result<()> {
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = line?;
        session.push("user", &line);
        let reply = format!("({} would answer here)", session.model);
        print_wrapped(&reply);
        session.push("assistant", &reply);
    }
    Ok(())
}

fn print_wrapped(text: &str) {
    let mut out = io::stdout().lock();
    for chunk in text.as_bytes().chunks(WIDTH) {
        let _ = writeln!(out, "{}", String::from_utf8_lossy(chunk));
    }
}
