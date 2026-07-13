//! Spotify Web API worker: library / search / browse + (later) playback control.
//! Hand-rolled over `ureq` + `serde_json` (same approach as `radio`/`archive`),
//! coalescing rapid searches latest-wins and backing off on 429 rate limits.
//! All metadata the UI shows — names, covers, artist photos, queue — comes from
//! here; librespot only produces audio.

const API: &str = "https://api.spotify.com/v1";

mod client;
mod playlist;
pub use client::{NOT_REGISTERED_MSG, enc, fmt_count, spawn};

/// A library section (the left sidebar of the Spotify view).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum Section {
    /// Spotify's editorial home feed (pathfinder GraphQL, via librespot — not the
    /// Web API). Distinct from the `/me/*` library sections below.
    Home,
    /// "Browse all" — the grid of genre/mood categories (pathfinder GraphQL).
    Browse,
    #[default]
    LikedSongs,
    Playlists,
    Albums,
    Artists,
    Podcasts,
    RecentlyPlayed,
    TopTracks,
}

impl Section {
    /// All sections in sidebar order.
    pub const ALL: [Section; 9] = [
        Section::Home,
        Section::Browse,
        Section::LikedSongs,
        Section::Playlists,
        Section::Albums,
        Section::Artists,
        Section::Podcasts,
        Section::RecentlyPlayed,
        Section::TopTracks,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Section::Home => "Home",
            Section::Browse => "Browse",
            Section::LikedSongs => "Liked Songs",
            Section::Playlists => "Playlists",
            Section::Albums => "Albums",
            Section::Artists => "Artists",
            Section::Podcasts => "Podcasts",
            Section::RecentlyPlayed => "Recently Played",
            Section::TopTracks => "Your Top Tracks",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Section::Home => "⌂",
            Section::Browse => "▦",
            Section::LikedSongs => "♥",
            Section::Playlists => "≡",
            Section::Albums => "◉",
            Section::Artists => "☻",
            Section::Podcasts => "▣",
            Section::RecentlyPlayed => "↺",
            Section::TopTracks => "★",
        }
    }
}

/// What an [`Item`] represents — drives the icon + what "open" does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Kind {
    #[default]
    Track,
    Album,
    Artist,
    Playlist,
    /// A podcast show (container → its episodes, which are playable `Track`s).
    Show,
    /// A "Browse all" category tile — opening it drills into that category's
    /// browse page (pathfinder GraphQL), not a track list.
    Category,
}

/// Section an item belongs to within a grouped artist page (`None` everywhere
/// else). The worker emits items already ordered Popular → Albums → Singles →
/// Compilations; the artist-page render maps these onto the shared section taxonomy
/// (`app::release::ReleaseSection`) for their header labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Group {
    #[default]
    None,
    Popular,
    Albums,
    Singles,
    Compilations,
}

/// A row in any Spotify list (library, search, playlist contents). Serializable
/// so the now-playing track + queue persist across sessions (see `session.rs`).
/// Metadata is kept as structured fields (artist/album/year/…) so the UI can
/// compose a row line or columns and toggle individual fields — see
/// `ui::components::item_meta`. `#[serde(default)]` on the newer fields lets
/// older sessions deserialize.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Item {
    /// `spotify:track:…` URI — used to start playback / queue.
    pub uri: String,
    pub name: String,
    /// Primary secondary field: artist(s) for tracks/albums, owner for a
    /// playlist, publisher for a show. Raw — the UI composes the rest.
    pub subtitle: String,
    /// Album name (tracks only; "" for containers) — for the Album column.
    pub album: String,
    /// Cover / artist image URL (smallest adequate size), if any.
    pub image: Option<String>,
    pub kind: Kind,
    /// Track length (ms) when known (tracks only).
    pub duration_ms: u32,
    /// `spotify:artist:…` of the primary artist (tracks only) — drives the rich
    /// artist pane.
    #[serde(default)]
    pub artist_uri: Option<String>,
    /// `spotify:show:…` of the parent show (podcast episodes only) — lets the pane
    /// open the show's page, the podcast analogue of `artist_uri`'s artist page.
    #[serde(default)]
    pub show_uri: Option<String>,
    /// Release year (tracks + albums).
    #[serde(default)]
    pub year: Option<u16>,
    /// Follower count (artists).
    #[serde(default)]
    pub followers: Option<u64>,
    /// Item count (playlists + shows: tracks / episodes).
    #[serde(default)]
    pub count: Option<u32>,
    /// Section within a grouped artist page (`None` elsewhere) — see [`Group`].
    #[serde(default)]
    pub group: Group,
    /// Free-form shelf title for multi-section browse (the Home feed), grouping
    /// items into labelled carousels. `None` for flat lists/grids. Distinct from
    /// [`Group`], which is the artist page's fixed release taxonomy.
    #[serde(default)]
    pub section: Option<String>,
    /// Packed `0xRRGGBB` background colour for a category tile (from Spotify's
    /// `cardRepresentation`), used to fill the grid card when it has no cover image —
    /// so a colour-only genre tile shows its brand colour, not a name placeholder.
    /// `None` for everything with a real cover.
    #[serde(default)]
    pub tint: Option<u32>,
}

impl Item {
    /// First listed artist of `subtitle` ("A, B, C" / "A · B" → "A"). Reads better
    /// against the lyrics/artist databases than the joined credit string.
    pub fn primary_artist(&self) -> &str {
        self.subtitle
            .split([',', '·'])
            .next()
            .unwrap_or(&self.subtitle)
            .trim()
    }
}

#[derive(Debug, Clone)]
pub enum SpRequest {
    /// Load one library section (the bearer `token` is passed in by the app).
    Library {
        section: Section,
        token: String,
        key: String,
    },
    /// Free-text search across tracks/albums/artists/playlists/shows.
    Search {
        query: String,
        token: String,
        key: String,
    },
    /// Drill into a container (album/playlist/artist) → its track list.
    Open {
        uri: String,
        kind: Kind,
        token: String,
        key: String,
    },
    /// Is this track in the user's Liked Songs? (for the ♥ indicator)
    CheckSaved { uri: String, token: String },
    /// Add/remove a track from Liked Songs.
    SetSaved {
        uri: String,
        saved: bool,
        token: String,
    },
    /// One artist's details (photo / genres / followers) for the artist pane.
    Artist {
        uri: String,
        token: String,
        key: String,
    },
    /// A podcast show's metadata (publisher + description) for the podcast artist
    /// pane's "About" — `GET /shows/{id}`. `uri` is `spotify:show:…`, echoed back.
    ShowMeta { uri: String, token: String },
    /// Follow/unfollow a podcast show (`user-library-modify`, `PUT/DELETE /me/shows`)
    /// or an artist (`user-follow-modify`, `PUT/DELETE /me/following`). The worker
    /// checks the current state and flips it, so the app needn't track it — `kind`
    /// selects the endpoint (only `Show`/`Artist` are valid).
    ToggleFollow {
        uri: String,
        kind: Kind,
        token: String,
    },
    /// Resolve a podcast episode's parent show URI (`GET /episodes/{id}` → `show.uri`)
    /// so the Artist pane can open the show even for an episode that predates
    /// `Item.show_uri` (e.g. one restored from an older session). `name` is echoed
    /// back to seed the breadcrumb.
    ResolveShow {
        episode_uri: String,
        name: String,
        token: String,
    },
    /// The account's OWN (owned / collaborative) playlists — the writable ones
    /// offered by the "add to playlist" picker. `user_id` filters out playlists the
    /// user merely follows (which a write would 403).
    MyPlaylists {
        token: String,
        user_id: String,
        key: String,
    },
    /// Create a new playlist (via `POST /me/playlists`), then add `uris` (may be
    /// empty).
    CreatePlaylist {
        token: String,
        name: String,
        uris: Vec<String>,
    },
    /// Add `uris` to an existing playlist (`name` is only for the confirmation toast).
    AddToPlaylist {
        token: String,
        playlist_uri: String,
        uris: Vec<String>,
        name: String,
    },
    /// Rename a playlist.
    RenamePlaylist {
        token: String,
        playlist_uri: String,
        name: String,
    },
    /// Replace a playlist's entire contents with `uris` (Spotify's "reorder or
    /// replace items" endpoint). lyrfin's *remove-a-track* path: personal apps can't
    /// DELETE playlist items, but they CAN replace the list with the current one
    /// minus the removed track. `uris` is capped at 100 (the caller guards).
    ReplacePlaylistItems {
        token: String,
        playlist_uri: String,
        uris: Vec<String>,
        name: String,
    },
    /// Unfollow ("delete") a playlist the user owns/follows.
    UnfollowPlaylist {
        token: String,
        playlist_uri: String,
        name: String,
    },
}

/// Which playlist write a [`SpResult::PlaylistWrite`] reports — drives what the
/// app refreshes on success (the Playlists section, the open track list, nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistOp {
    Create,
    Add,
    Rename,
    Remove,
    Unfollow,
}

#[derive(Debug, Clone)]
pub enum SpResult {
    Library {
        key: String,
        items: Vec<Item>,
    },
    Search {
        key: String,
        tracks: Vec<Item>,
        albums: Vec<Item>,
        artists: Vec<Item>,
        playlists: Vec<Item>,
        shows: Vec<Item>,
    },
    /// Tracks of an opened container (album/playlist/artist top tracks).
    Opened {
        key: String,
        items: Vec<Item>,
    },
    /// Liked-Songs state of a track (after a check or a successful toggle).
    Saved {
        uri: String,
        saved: bool,
    },
    /// One artist's details for the pane (keyed by the artist `uri` it answers).
    Artist {
        uri: String,
        name: String,
        image: Option<String>,
        genres: String,
        followers: u64,
    },
    /// A podcast show's metadata for the pane's "About" (keyed by the show `uri`).
    ShowMeta {
        uri: String,
        publisher: String,
        description: String,
    },
    /// Follow state of a show/artist after a successful toggle (`followed` = the new
    /// state). Keyed by `uri` so the app can match it to the pending toast.
    Follow {
        uri: String,
        followed: bool,
    },
    /// A podcast episode's parent show, resolved for the Artist pane to open. `uri`
    /// is `None` when the lookup failed; `name` seeds the breadcrumb.
    ShowResolved {
        uri: Option<String>,
        name: String,
    },
    /// The account's writable playlists for the add picker (keyed to the modal's
    /// open request; `error` is set instead of `items` on a fetch failure).
    MyPlaylists {
        key: String,
        items: Vec<Item>,
        error: Option<String>,
    },
    /// Outcome of a playlist WRITE: the user-facing toast + whether it succeeded +
    /// which op (so the app knows what to refresh). Self-contained — never routed
    /// through the browse `key` machinery.
    PlaylistWrite {
        op: PlaylistOp,
        ok: bool,
        msg: String,
    },
    /// The token was rejected (401) — the app should refresh + retry.
    Unauthorized {
        key: String,
    },
    Error {
        key: String,
        msg: String,
    },
}
