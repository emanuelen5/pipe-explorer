//! Drawing the TUI, and the key presses that cause a redraw.
//!
//! Each benchmark is run against outputs of different sizes.  The time to draw
//! a frame should not depend on how much output there is, since only one
//! screenful is ever visible: if the larger size is noticeably slower,
//! something is scanning the whole output on every frame (see issue #48).

mod common;

use common::{Flavor, SIZES, TERM_HEIGHT, TERM_WIDTH, size_label, stage_output, text};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use pipe_explorer::app::{App, OutputMode};
use pipe_explorer::executor::StageOutput;
use pipe_explorer::pipeline::parse_pipeline;
use pipe_explorer::ui;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tokio::runtime::Runtime;

/// Every n-th line goes to stderr (only visible in combined mode).
const STDERR_EVERY: usize = 10;

/// An app showing one finished stage, drawn into an in-memory terminal.
struct Fixture {
    app: App,
    terminal: Terminal<TestBackend>,
    // `App::new` spawns its executor task, so it needs a runtime to live in.
    _runtime: Runtime,
}

impl Fixture {
    /// Show `output` in `mode`, scrolled to the middle.
    fn new(output: StageOutput, mode: OutputMode) -> Self {
        let runtime = Runtime::new().expect("tokio runtime");
        let mut app = {
            let _guard = runtime.enter();
            App::new(parse_pipeline("find ."))
        };
        let lines = output.display_line_count(mode);
        app.stage_outputs = vec![output];
        app.view_mut().output_mode = mode;
        app.view_mut().scroll = lines / 2;

        let terminal = Terminal::new(TestBackend::new(TERM_WIDTH, TERM_HEIGHT)).expect("terminal");
        let mut fixture = Self {
            app,
            terminal,
            _runtime: runtime,
        };
        // The first frame lets the app learn the size of the output pane.
        fixture.draw();
        fixture
    }

    fn draw(&mut self) {
        let app = &mut self.app;
        self.terminal
            .draw(|frame| ui::render(frame, app))
            .expect("draw");
    }

    /// Press `key` and draw the resulting frame, like the event loop does.
    fn press(&mut self, key: KeyCode) {
        self.app
            .handle_event(Event::Key(KeyEvent::new(key, KeyModifiers::NONE)));
        self.draw();
    }
}

/// Drawing a frame without any input, for each kind of output.
fn frame(c: &mut Criterion) {
    let mut group = c.benchmark_group("render/frame");
    let cases = [
        ("plain", Flavor::Plain, OutputMode::Stdout),
        ("ansi", Flavor::Ansi, OutputMode::Stdout),
        // Wrapped lines make the max-scroll computation work harder.
        ("long", Flavor::Long, OutputMode::Stdout),
        ("combined", Flavor::Plain, OutputMode::Combined),
    ];
    for (label, flavor, mode) in cases {
        for n in SIZES {
            let mut fixture = Fixture::new(stage_output(&text(flavor, n), STDERR_EVERY), mode);
            group.bench_function(BenchmarkId::new(label, size_label(n)), |b| {
                b.iter(|| fixture.draw())
            });
        }
    }
    group.finish();
}

/// Key presses that only change the view, each followed by a redraw.
fn keypress(c: &mut Criterion) {
    let mut group = c.benchmark_group("render/keypress");
    for n in SIZES {
        let output = || stage_output(&text(Flavor::Plain, n), STDERR_EVERY);

        // Moving the cursor in the command editor (issue #48).
        let mut fixture = Fixture::new(output(), OutputMode::Stdout);
        fixture.app.start_editing();
        fixture.draw();
        let mut left = true;
        group.bench_function(BenchmarkId::new("editor_cursor", size_label(n)), |b| {
            b.iter(|| {
                left = !left;
                fixture.press(if left { KeyCode::Left } else { KeyCode::Right });
            })
        });

        // Scrolling the output pager.
        let mut fixture = Fixture::new(output(), OutputMode::Stdout);
        let mut down = true;
        group.bench_function(BenchmarkId::new("scroll", size_label(n)), |b| {
            b.iter(|| {
                down = !down;
                fixture.press(if down {
                    KeyCode::Char('j')
                } else {
                    KeyCode::Char('k')
                });
            })
        });

        // Jumping between search matches (one match per line).
        let mut fixture = Fixture::new(output(), OutputMode::Stdout);
        fixture.app.view_mut().search.query = "file".to_string();
        fixture.app.compute_search_matches();
        let middle = fixture.app.view().search.matches.len() / 2;
        fixture.app.view_mut().search.match_idx = middle;
        fixture.app.view_mut().scroll = fixture.app.view().search.matches[middle].0;
        fixture.draw();
        let mut next = true;
        group.bench_function(BenchmarkId::new("search_next", size_label(n)), |b| {
            b.iter(|| {
                next = !next;
                fixture.press(if next {
                    KeyCode::Char('n')
                } else {
                    KeyCode::Char('N')
                });
            })
        });
    }
    group.finish();
}

criterion_group!(benches, frame, keypress);
criterion_main!(benches);
