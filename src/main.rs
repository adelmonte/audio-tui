//! audio-tui — a modern terminal mixer and control panel for PipeWire.

mod action;
mod app;
mod backend;
mod cfgedit;
mod config;
mod core;
mod ui;
mod view;

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use crossterm::event::EventStream;
use futures::StreamExt;

use crate::app::App;
use crate::backend::conf::Dirs;
use crate::config::{Config, APP, DEFAULT_TOML};

#[derive(Parser, Debug)]
#[command(name = "audio-tui", version, about = "Control PipeWire audio from the terminal")]
struct Cli {
    /// Use a fake audio graph (no PipeWire needed). System files go to a temp folder.
    #[arg(long)]
    demo: bool,
    /// Config file (default: ~/.config/audio-tui/config.toml)
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Print the default config and exit
    #[arg(long)]
    print_default_config: bool,
    /// Write the system drop-ins from config.toml and exit (no restart)
    #[arg(long)]
    apply: bool,
    /// Verbose logging to the log file
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    if cli.print_default_config {
        print!("{DEFAULT_TOML}");
        return Ok(());
    }
    let _log = core::logging::init(APP, cli.verbose);
    let loaded = core::config::load::<Config>(APP, cli.config.as_deref(), DEFAULT_TOML);
    let dirs = if cli.demo {
        let root = std::env::temp_dir().join("audio-tui-demo");
        Dirs { pipewire: root.join("pipewire.conf.d"), pulse: root.join("pipewire-pulse.conf.d"), client: root.join("client.conf.d"), wireplumber: root.join("wireplumber.conf.d") }
    } else {
        Dirs::user()
    };
    if cli.apply {
        let (files, errors) = backend::conf::generate(&loaded.value, &dirs);
        for e in errors {
            eprintln!("warning: {e}");
        }
        for (path, change, svc) in backend::conf::apply(&files)? {
            println!("{change:?} {} (restart {:?})", path.display(), svc);
        }
        return Ok(());
    }

    let peaks = backend::Peaks::default();
    let mut backend = if cli.demo { backend::demo::spawn(peaks.clone()) } else { backend::pw::spawn(peaks.clone()) };
    let (app_tx, mut app_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(loaded.value, loaded.path, backend.tx.clone(), app_tx, peaks, cli.demo, loaded.warnings, dirs);
    if app.cfg.privacy.prune_titles_on_start {
        app.prune_titles();
    }

    let mut term = core::term::init(app.cfg.general.mouse)?;
    let mut events = EventStream::new();
    // ~30 fps so meters move smoothly; idle cost is tiny (ratatui only diffs).
    let mut tick = tokio::time::interval(Duration::from_millis(33));
    let result: color_eyre::Result<()> = async {
        loop {
            term.draw(|f| view::render(&mut app, f))?;
            app.sync_meters();
            tokio::select! {
                e = events.next() => match e {
                    Some(Ok(e)) => app.on_event(e),
                    Some(Err(e)) => return Err(e.into()),
                    None => break,
                },
                Some(ev) = backend.rx.recv() => {
                    app.on_backend(ev);
                    while let Ok(ev) = backend.rx.try_recv() {
                        app.on_backend(ev);
                    }
                }
                Some(ev) = app_rx.recv() => app.on_app_event(ev),
                _ = tick.tick() => app.on_tick(),
            }
            if app.quit {
                break;
            }
        }
        Ok(())
    }
    .await;
    core::term::restore();
    result
}
