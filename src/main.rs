mod ansi;
mod app;
mod executor;
mod pipeline;
mod search;
mod ui;

use clap::Parser;

use app::{App, run};
use pipeline::Pipeline;

const LONG_ABOUT: &str = concat!(
    "Build and inspect shell pipelines interactively",
    " and look at the output of each stage (command between each pipe)",
    " while they execute.",
);

/// Pipe Explorer — build and inspect shell pipe commands interactively.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = LONG_ABOUT, trailing_var_arg = true)]
struct Args {
    /// Shell pipeline to start with. E.g.
    /// - `pipe-explorer echo hello \| grep hello` creates a pipeline with two stages.
    /// If you pass `--parse`, then each argument is further split into stages using " | " as a separator. E.g.
    /// - `pipe-explorer --parse "echo hello | grep hello"` creates a pipeline with two stages: `echo hello` and `grep hello`.
    #[arg(value_name = "CMD", verbatim_doc_comment)]
    cmds: Vec<String>,

    /// If set, it will parse each argument (CMD) into stages using " | " as a
    /// separator
    #[arg(long, short)]
    parse: bool,
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
    let pipeline = Pipeline::from_commands(args.cmds, args.parse);

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
