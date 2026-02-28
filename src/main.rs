mod app;
mod ansi;
mod executor;
mod pipeline;
mod search;
mod ui;

use clap::Parser;

use app::{App, run};
use pipeline::parse_pipeline;

/// Pipe Explorer — build and inspect shell pipe commands interactively.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Shell pipeline to start with (e.g. "echo hello | grep hello").
    /// Stages are separated by " | " (with spaces around the pipe character).
    #[arg(value_name = "PIPELINE")]
    pipeline: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install a panic hook that restores the terminal before printing the panic.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        default_hook(info);
    }));

    let args = Args::parse();

    let pipeline = match args.pipeline {
        Some(ref s) => parse_pipeline(s),
        None => pipeline::Pipeline::new(vec![]),
    };

    let mut app = App::new(pipeline);
    let result = run(&mut app).await;

    println!(
        "{}",
        app.pipeline
            .stages
            .iter()
            .map(|s| s.command.as_str())
            .collect::<Vec<_>>()
            .join(" | ")
    );
    result
}
