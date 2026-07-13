//! lyrfin — a modern, futuristic terminal music player (TUI).
//!
//! Data flow:  Event  ->  Action  ->  AppState::update  ->  render
//! See `docs/ARCHITECTURE.md`.

mod action;
mod app;
mod appearance;
mod arabic;
mod artistinfo;
mod artwork;
mod audio;
mod config;
mod core;
mod cover;
mod coversearch;
mod datetime;
mod event;
mod icons;
mod keymap;
mod library;
mod lyrics;
mod lyricsfetch;
mod media;
mod podcastfetch;
mod query;
mod radio;
mod session;
mod snapshot;
mod spotify;
mod stats;
mod tags;
mod tagsearch;
mod termquery;
mod translate;
mod tui;
mod ui;

use std::path::PathBuf;

use app::AppState;
use config::Config;

/// A modern, futuristic terminal music player.
#[derive(Debug)]
struct Cli {
    /// Music files or directories to add to the library for this session.
    paths: Vec<PathBuf>,
    /// Theme name (overrides config).
    theme: Option<String>,
    /// Render every layout headlessly (no TTY) and exit — for testing.
    snapshot: bool,
    /// Terminal size for --snapshot, e.g. 120x40.
    size: String,
}

/// Minimal hand-rolled arg parser (avoids the clap dependency for 4 args).
/// Accepts `--flag value` and `--flag=value`; bare args are music paths.
fn parse_args() -> Cli {
    let mut cli = Cli {
        paths: Vec::new(),
        theme: None,
        snapshot: false,
        size: "120x40".to_string(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f.to_string(), Some(v.to_string())),
            None => (arg.clone(), None),
        };
        let mut value = || inline.clone().or_else(|| args.next());
        match flag.as_str() {
            "--theme" => cli.theme = value(),
            "--size" => {
                if let Some(v) = value() {
                    cli.size = v;
                }
            }
            "--snapshot" => cli.snapshot = true,
            "-h" | "--help" => {
                println!(
                    "lyrfin {}\n\nUsage: lyrfin [PATHS]... [--theme NAME] [--snapshot] [--size WxH]",
                    env!("CARGO_PKG_VERSION")
                );
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("lyrfin {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other if other.starts_with('-') => {
                eprintln!("lyrfin: unknown option '{other}' (try --help)");
            }
            _ => cli.paths.push(PathBuf::from(arg)),
        }
    }
    cli
}

fn main() -> anyhow::Result<()> {
    // rustls 0.23 needs a process-wide CryptoProvider; the dep tree enables more
    // than one backend so it can't auto-pick. Install ring before librespot's TLS
    // (otherwise the session thread panics on the first connection).
    let _ = rustls::crypto::ring::default_provider().install_default();
    // Install the log hook: it honours `RUST_LOG` (librespot + lyrfin logging to
    // stderr — the TUI uses stdout, so `RUST_LOG=librespot=info lyrfin 2>/tmp/lyrfin.log`
    // captures it cleanly) AND always watches for librespot's audio-key denial so
    // playback that Spotify blocks is reported instead of buffering forever.
    spotify::logprobe::init();
    let cli = parse_args();

    let mut config = Config::load_or_default();
    // The private Spotify client id lives in its OWN file so config.toml churn can
    // never wipe it. Prefer it; migrate a legacy config.toml value the first time.
    let persisted = spotify::auth::load_persisted_client_id(&config.dir);
    let client_id = persisted
        .clone()
        .unwrap_or_else(|| config.spotify_client_id.clone());
    if persisted.is_none() && !client_id.is_empty() {
        spotify::auth::persist_client_id(&config.dir, &client_id); // migrate + apply
    } else {
        spotify::auth::set_client_id(client_id.clone());
    }
    config.spotify_client_id = client_id; // keep the in-memory mirror in sync
    if let Some(t) = &cli.theme {
        config.theme = t.clone();
        // an explicit --theme is a single-theme override → don't let follow-system
        // (if enabled in the saved config) switch away from it this run
        config.theme_follows_system = false;
    }
    if !cli.paths.is_empty() {
        // dirs become scan roots; a file's parent dir is scanned too
        config.music_dirs = cli
            .paths
            .iter()
            .map(|p| {
                if p.is_file() {
                    p.parent().map(PathBuf::from).unwrap_or_else(|| p.clone())
                } else {
                    p.clone()
                }
            })
            .collect();
    }

    let mut app = AppState::new(config);
    // Load the cached catalogue for an instant start (the background sync in
    // `tui` reconciles it with disk). On a fresh install the cache is empty and we
    // leave the library empty on purpose: the UI shows the first-run onboarding
    // (see `components::welcome`) instead of fabricated demo tracks. `seed_demo`
    // lives on for tests and `--snapshot` screenshots.
    let cache = library::store::LibraryCache::load(&app.config.dir);
    app.load_cached_library(cache.tracks);
    // restore last session (view, highlights, last-played + queue), unless --theme set
    let restore = session::Session::load(&app.config.dir);
    app.apply_session(restore);
    // optimistic Spotify view: show the browse list left on last time instantly, so
    // the pane isn't blank while librespot reconnects + re-fetches behind it (the
    // network refresh swaps it in place; a wrong-account cache is dropped on connect)
    app.spotify_apply_view_cache();
    if let Some(t) = &cli.theme {
        app.config.theme = t.clone();
        app.theme = crate::ui::theme::Theme::resolve(t, &app.config.themes_dir());
    }

    if cli.snapshot {
        let (w, h) = cli
            .size
            .split_once('x')
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            .unwrap_or((120, 40));
        snapshot::dump_all(&mut app, w, h);
        return Ok(());
    }

    tui::run(app)
}
