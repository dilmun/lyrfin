//! The music library data model.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// An interned string: a cheap-to-clone `Arc<str>`. The same artist/album/genre
/// text is stored once and shared across every track that uses it (see
/// `Library::from_tracks`, which dedups), instead of an owned `String` per track.
/// Behaves like `&str`/`String` at call sites (Deref + the PartialEq/From impls).
#[derive(Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IStr(Arc<str>);

impl IStr {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl std::ops::Deref for IStr {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}
impl AsRef<str> for IStr {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
impl From<String> for IStr {
    fn from(s: String) -> Self {
        IStr(Arc::from(s.into_boxed_str()))
    }
}
impl From<&str> for IStr {
    fn from(s: &str) -> Self {
        IStr(Arc::from(s))
    }
}
impl From<&String> for IStr {
    fn from(s: &String) -> Self {
        IStr(Arc::from(s.as_str()))
    }
}
impl std::fmt::Display for IStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::fmt::Debug for IStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&*self.0, f)
    }
}
impl PartialEq<str> for IStr {
    fn eq(&self, o: &str) -> bool {
        &*self.0 == o
    }
}
impl PartialEq<&str> for IStr {
    fn eq(&self, o: &&str) -> bool {
        &*self.0 == *o
    }
}
impl PartialEq<String> for IStr {
    fn eq(&self, o: &String) -> bool {
        &*self.0 == o.as_str()
    }
}
impl Serialize for IStr {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for IStr {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(IStr::from(String::deserialize(d)?))
    }
}

/// A 1-based entity id. `NonZeroU32` (not `u64`) so the id is 4 bytes and, via
/// the null-pointer-style niche, `Option<Id>` is *also* 4 bytes — `Track`'s
/// `album_id`/`artist_id` cost 4 bytes each instead of 16. Ids are assigned
/// sequentially from 1, so zero is never a valid id (it doubles as the niche).
macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        pub struct $name(pub std::num::NonZeroU32);

        impl $name {
            /// Build from a raw 1-based id. Panics on 0 (ids are never zero).
            #[track_caller]
            pub const fn new(n: u32) -> Self {
                match std::num::NonZeroU32::new(n) {
                    Some(v) => $name(v),
                    None => panic!("entity id must be non-zero"),
                }
            }
            /// The underlying numeric id. (Idiomatic accessor; not all id types
            /// read it today.)
            #[allow(dead_code)]
            pub const fn get(self) -> u32 {
                self.0.get()
            }
        }
    };
}
id_type!(TrackId);
id_type!(AlbumId);
id_type!(ArtistId);
id_type!(PlaylistId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Codec {
    Flac,
    Alac,
    Mp3,
    Aac,
    OggVorbis,
    Opus,
    Wav,
    Other,
}

impl Codec {
    pub fn name(self) -> &'static str {
        match self {
            Codec::Flac => "FLAC",
            Codec::Alac => "ALAC",
            Codec::Mp3 => "MP3",
            Codec::Aac => "AAC",
            Codec::OggVorbis => "OGG",
            Codec::Opus => "Opus",
            Codec::Wav => "WAV",
            Codec::Other => "Audio",
        }
    }
    pub fn is_lossless(self) -> bool {
        matches!(self, Codec::Flac | Codec::Alac | Codec::Wav)
    }
}

/// Technical audio properties shown in the "format" chip.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AudioInfo {
    pub codec: Codec,
    pub sample_rate: u32, // Hz
    pub bit_depth: u8,    // 0 = lossy
    pub channels: u8,
    pub bitrate_kbps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub path: PathBuf,
    pub title: String,
    pub artist: IStr,
    pub album: IStr,
    pub album_artist: IStr,
    pub album_id: Option<AlbumId>,
    pub artist_id: Option<ArtistId>,
    pub track_no: u16,
    pub disc_no: u16,
    #[serde(default)]
    pub track_total: u16,
    #[serde(default)]
    pub disc_total: u16,
    /// Track length in milliseconds (u32 ≈ 49 days max — `Duration` is 16 bytes,
    /// this is 4). Read it as a `Duration` via [`Track::duration`].
    pub duration_ms: u32,
    pub year: Option<u16>,
    pub genre: Option<IStr>,
    #[serde(default)]
    pub composer: String,
    #[serde(default)]
    pub comment: String,
    pub audio: Option<AudioInfo>,
    // user data (persisted in the library store, M3)
    pub rating: u8, // 0..=5
    pub favorite: bool,
    pub play_count: u32,
    pub added_at: u32, // unix seconds (u32: valid to year 2106)
    #[serde(default)]
    pub last_played: u32, // unix seconds; 0 = never (or pre-dates this field)
}

/// Domain aggregate; some fields (artist/genre/cover_path) are populated by
/// indexing but not yet surfaced in any view.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Album {
    pub id: AlbumId,
    pub title: String,
    pub artist: String,
    pub artist_id: Option<ArtistId>,
    pub year: Option<u16>,
    pub genre: Option<String>,
    pub track_ids: Vec<TrackId>,
    /// Path to embedded/extracted cover, populated lazily.
    pub cover_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Artist {
    pub id: ArtistId,
    pub name: String,
    pub album_ids: Vec<AlbumId>,
}

#[derive(Debug, Clone)]
pub struct Playlist {
    pub id: PlaylistId,
    pub name: String,
    pub track_ids: Vec<TrackId>,
    /// Smart (dynamic) playlist: when set, membership is computed live from this
    /// query (see `crate::query`) instead of the static `track_ids`.
    pub query: Option<String>,
    /// Optional folder this playlist is grouped under in the sidebar.
    pub folder: Option<String>,
}

impl Track {
    /// Rating clamped to 0..=5 (for a star renderer); not wired to a view yet.
    #[allow(dead_code)]
    pub fn stars(&self) -> u8 {
        self.rating.min(5)
    }
    /// Track length as a `Duration` (stored compactly as `duration_ms`).
    pub fn duration(&self) -> Duration {
        Duration::from_millis(self.duration_ms as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    /// Guards the field-shrink win (audit N2): `NonZeroU32` ids give `Option<Id>`
    /// the null-niche, and `duration`/timestamps are `u32` — so these stay small.
    /// If a field change reinflates them, this fails loudly.
    #[test]
    fn ids_use_the_nonzero_niche() {
        assert_eq!(size_of::<TrackId>(), 4);
        assert_eq!(
            size_of::<Option<TrackId>>(),
            4,
            "niche: no discriminant word"
        );
        assert_eq!(size_of::<Option<AlbumId>>(), 4);
        assert_eq!(size_of::<Option<ArtistId>>(), 4);
    }
}
