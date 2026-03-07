mod ansi;
mod app;
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
async fn main() {
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

    // Print the pipeline command with mode-aware connectors so the user can
    // copy-paste it into a terminal.
    let stages = &app.pipeline.stages;
    let mut cmd = String::new();
    for (i, stage) in stages.iter().enumerate() {
        cmd.push_str(&stage.command);
        if i + 1 < stages.len() {
            let mode = app
                .stage_views
                .get(i)
                .map(|v| v.output_mode)
                .unwrap_or(app::OutputMode::Stdout);
            let connector = ui::pipe_connector(mode);
            cmd.push_str(connector);
        }
    }
    println!("{}", cmd);

    // By using exit we ensure that any background processes that still are
    // executing are killed immediately
    std::process::exit(if result.is_ok() { 0 } else { 1 });
}
