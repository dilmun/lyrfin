//! Session persistence: remember the full UI + playback state between launches
//! so lyrfin resumes exactly where you left off (view/layout, highlight positions,
//! expanded tree, last-played track + position, and the queue). Stored as
//! `session.json` in the config dir, separate from the user-authored `config.toml`.
//!
//! Library-dependent fields are stored by **file path / artist name** (not
//! TrackId, which is reassigned on each scan) so they survive a rescan.

use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize};

/// Deserialize a field, but fall back to its `Default` if the value doesn't fit the
/// current type instead of failing the *whole* `Session`. This keeps a session
/// readable across builds: if a nested type (e.g. `Item`) gains/changes a field, the
/// old `session.json` loses only that one field — never the entire session (layout,
/// panels, queue, …). Used on every field whose type can drift between versions.
fn lenient<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).unwrap_or_default())
}

/// Serialized per-view panel: `(layout, panel, shown, dock, size)`.
pub type PanelSer = (String, String, bool, String, u16);

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    // appearance / view. NOTE: the theme is NOT persisted here — it's a setting
    // that lives in config.toml (saved on every theme change), so a single source
    // of truth. A stale `theme` key in an old session.json is ignored on load.
    pub volume: Option<u8>,
    pub layout: Option<String>,
    pub focus: Option<String>,
    /// Per-view visualizer mode: `(layout, mode)`.
    pub visualizer_modes: Option<Vec<(String, u8)>>,
    /// Per-view panel state.
    pub panels: Option<Vec<PanelSer>>,
    /// Per-panel cross-axis height/width share `(layout, panel, len)`. A separate
    /// list (not part of `PanelSer`) so older sessions stay readable — a missing
    /// entry just means the default even split.
    pub pane_lens: Option<Vec<(String, String, u16)>>,
    /// Per-section grid/list override `(section_key, grid)` — the `#` toggle.
    pub grid_sections: Option<Vec<(String, bool)>>,

    // playback modes
    pub shuffle: Option<bool>,
    pub repeat: Option<String>, // "off" | "all" | "one"
    pub speed: Option<f32>,

    // highlight positions
    pub selection: Option<usize>,
    pub queue_sel: Option<usize>,
    /// Local library drill-in: the selected section, the open-container drill path
    /// (stable name-based refs), + the cursor — restored on reopen.
    pub local_section: Option<String>,
    pub local_open: Option<Vec<String>>,
    pub local_sel: Option<usize>,

    // library-dependent (resolved by path on load)
    pub current_path: Option<String>, // last-played track
    pub elapsed_secs: Option<u64>,    // position within it
    pub queue_paths: Option<Vec<String>>,

    // internet radio: the active filters + the last-tuned station
    pub radio_section: Option<String>, // active sidebar section (RadioSection key)
    pub radio_query: Option<String>,
    pub radio_country: Option<(String, String)>, // (name, ISO code)
    pub radio_tag: Option<String>,
    pub radio_sort: Option<String>, // sort label
    #[serde(default, deserialize_with = "lenient")]
    pub radio_station: Option<crate::radio::Station>,

    // Spotify: the account (id) the saved state belongs to, so it's never restored
    // onto a different account on the next launch (checked on reconnect).
    pub spotify_account: Option<String>,

    // Spotify: the last now-playing track + its queue, restored as a paused
    // overlay (space re-loads it at `spotify_pos`, like a restored radio station).
    #[serde(default, deserialize_with = "lenient")]
    pub spotify_now: Option<crate::spotify::api::Item>,
    #[serde(default, deserialize_with = "lenient")]
    pub spotify_queue: Option<Vec<crate::spotify::api::Item>>,
    pub spotify_idx: Option<usize>,
    pub spotify_pos: Option<f64>, // elapsed seconds within the track
    /// Spotify queue playback modes, restored as flags (the queue is already saved in
    /// its shuffled order, so restore does NOT re-shuffle). Separate from the local
    /// player's `shuffle`/`repeat` above — the two overlays keep independent modes.
    pub spotify_shuffle: Option<bool>,
    pub spotify_repeat: Option<String>, // "off" | "all" | "one"

    // Spotify: the last browse view (re-fetched on reconnect — see spotify.rs)
    #[serde(default, deserialize_with = "lenient")]
    pub spotify_section: Option<crate::spotify::api::Section>,
    pub spotify_sel: Option<usize>,      // cursor in the list
    pub spotify_query: Option<String>,   // search box text
    pub spotify_in_search: Option<bool>, // showing search results vs the section
    /// A drilled-into container (playlist/album/artist) — re-opened on reconnect.
    /// Stored as [`SpotifyDrill`] (stable primitives: URI + `Kind` + name), NOT the
    /// whole `api::Item` (whose many fields drift across builds), so an `Item` schema
    /// change on a new binary can't drop the restored drill-in (which would land the
    /// user back at the section). Carrying the kind + name — not just the URI — lets
    /// the container re-open even when it isn't in the reloaded section list (e.g. an
    /// artist opened from the now-playing track). A prior build persisted this as a
    /// bare URI string, which `lenient` degrades to `None` for one launch; an even
    /// older `Item`-object value migrates cleanly, since it shares the uri/kind/name
    /// fields (the rest of the `Item` is ignored).
    #[serde(default, deserialize_with = "lenient")]
    pub spotify_open: Option<SpotifyDrill>,
    /// Spotify per-section cover-grid/list overrides (the `#` toggle), by section.
    #[serde(default, deserialize_with = "lenient")]
    pub spotify_grid_sections: Option<Vec<(crate::spotify::api::Section, bool)>>,
}

/// A persisted Spotify drill-in: just enough of the container to re-open it on
/// reconnect — its stable URI, its [`Kind`](crate::spotify::api::Kind) (which fetch
/// path to use), and its display name (the breadcrumb). Deliberately NOT the whole
/// `api::Item`: those are stable across builds where the rich `Item` (image, counts,
/// year, …) is not, and they suffice to re-open the container even when it isn't in
/// the reloaded section list. Every field defaults so the struct stays readable as it
/// evolves.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SpotifyDrill {
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub kind: crate::spotify::api::Kind,
    #[serde(default)]
    pub name: String,
}

impl Session {
    pub fn load(dir: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(dir.join("session.json")) else {
            return Self::default(); // first run / no session yet
        };
        match serde_json::from_str(&text) {
            Ok(s) => s,
            // Per-field `lenient` deserialization means a normal schema change can't
            // reach here (only a hard top-level corruption can) — note it rather than
            // silently resuming blank, so it's not mistaken for "nothing was saved".
            Err(e) => {
                eprintln!("lyrfin: couldn't read session.json ({e}); starting fresh");
                Self::default()
            }
        }
    }

    pub fn save(&self, dir: &Path) {
        let Ok(json) = serde_json::to_string_pretty(self) else {
            return;
        };
        let _ = std::fs::create_dir_all(dir);
        // atomic write (temp + rename) so a crash/kill mid-write can't truncate
        // session.json into something the next launch fails to parse
        let tmp = dir.join("session.json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join("session.json"));
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_broken_nested_field_does_not_wipe_the_whole_session() {
        // simulate a new build where `Item` changed shape: an old session.json with a
        // valid layout/panels but a `spotify_now` that no longer fits the type. The
        // whole session must NOT collapse to default — only the broken field drops.
        let json = r#"{
            "layout": "spotify",
            "volume": 70,
            "panels": [["spotify", "queue", true, "right", 26]],
            "spotify_now": { "kind": 99999, "totally": "the wrong shape" }
        }"#;
        let s: Session = serde_json::from_str(json).expect("session still parses (lenient)");
        assert_eq!(s.layout.as_deref(), Some("spotify"), "layout survives");
        assert_eq!(s.volume, Some(70), "volume survives");
        assert!(
            s.panels.as_ref().is_some_and(|p| !p.is_empty()),
            "panels survive a broken nested Item field"
        );
        assert!(
            s.spotify_now.is_none(),
            "only the broken field is dropped, not the whole session"
        );
    }

    #[test]
    fn pre_migration_spotify_open_never_wipes_the_session() {
        // `spotify_open` has changed shape (old `Item` object → interim bare-URI string
        // → `SpotifyDrill`). Whatever a legacy session holds, the rest of the session
        // must stay readable (via `lenient`) — never a whole-parse failure that wipes
        // layout/panels/queue with it.
        for legacy in [
            r#"{ "uri": "spotify:artist:1", "name": "x", "subtitle": "y", "image": null }"#,
            r#""spotify:artist:1""#, // interim URI string
            r#"12345"#,              // pure garbage
        ] {
            let json =
                format!(r#"{{ "layout": "spotify", "volume": 70, "spotify_open": {legacy} }}"#);
            let s: Session = serde_json::from_str(&json).expect("session still parses (lenient)");
            assert_eq!(
                s.layout.as_deref(),
                Some("spotify"),
                "layout survives ({legacy})"
            );
            assert_eq!(s.volume, Some(70), "volume survives ({legacy})");
        }
        // the interim bare-URI form can't be a struct → dropped cleanly, not mis-read
        let s: Session = serde_json::from_str(r#"{ "spotify_open": "spotify:artist:1" }"#).unwrap();
        assert!(
            s.spotify_open.is_none(),
            "a bare URI string degrades to None"
        );
    }

    #[test]
    fn spotify_open_drill_round_trips() {
        // A SpotifyDrill is stable primitives (uri/kind/name) — it round-trips
        // regardless of `api::Item` drift, and carries enough to re-open off-list.
        use crate::spotify::api::Kind;
        let dir = std::env::temp_dir().join("lyrfin_session_spotify_open");
        let _ = std::fs::remove_dir_all(&dir);
        let s = Session {
            spotify_open: Some(SpotifyDrill {
                uri: "spotify:artist:37i9".into(),
                kind: Kind::Artist,
                name: "Rahma Riad".into(),
            }),
            ..Session::default()
        };
        s.save(&dir);
        let back = Session::load(&dir).spotify_open.expect("drill round-trips");
        assert_eq!(back.uri, "spotify:artist:37i9");
        assert_eq!(back.kind, Kind::Artist);
        assert_eq!(back.name, "Rahma Riad");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_is_atomic_and_round_trips() {
        let dir = std::env::temp_dir().join("lyrfin_session_atomic");
        let _ = std::fs::remove_dir_all(&dir);
        let s = Session {
            layout: Some("spotify".into()),
            volume: Some(55),
            ..Session::default()
        };
        s.save(&dir);
        assert!(
            !dir.join("session.json.tmp").exists(),
            "the temp file is renamed away — none left behind"
        );
        let back = Session::load(&dir);
        assert_eq!(back.layout.as_deref(), Some("spotify"));
        assert_eq!(back.volume, Some(55));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
