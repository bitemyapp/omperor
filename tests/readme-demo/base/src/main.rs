mod chat;
mod session;

use anyhow::Result;

fn main() -> Result<()> {
    let model = std::env::args().nth(2).unwrap_or_else(|| "llama3".into());
    let mut session = session::Session::open(&model)?;
    chat::run(&mut session)
}
