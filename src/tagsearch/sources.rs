//! Online metadata-source clients for tag search, extracted from `tagsearch`:
//! iTunes + Deezer (single song + whole album) and MusicBrainz (album only —
//! it carries the Deluxe/regional editions the other two often lack). Each
//! returns `TagCandidate`s / `AlbumMatch`es ranked against the local track.

use super::*;

/// iTunes song search — the richest source (year / genre / track + disc).
pub(crate) fn itunes(agent: &ureq::Agent, query: &str, out: &mut Vec<TagCandidate>) {
    let _ = (|| -> Option<()> {
        let v: Value = agent
            .get("https://itunes.apple.com/search")
            .header("User-Agent", UA)
            .query("term", query)
            .query("entity", "song")
            .query("limit", "10")
            .call()
            .ok()?
            .body_mut()
            .read_json()
            .ok()?;
        let s = |r: &Value, k: &str| r.get(k).and_then(Value::as_str).map(str::to_string);
        let n = |r: &Value, k: &str| r.get(k).and_then(Value::as_u64).map(|x| x as u16);
        for r in v.get("results")?.as_array()? {
            let title = s(r, "trackName").unwrap_or_default();
            if title.is_empty() {
                continue;
            }
            out.push(TagCandidate {
                source: "iTunes",
                title,
                artist: s(r, "artistName").unwrap_or_default(),
                album: s(r, "collectionName").unwrap_or_default(),
                album_artist: s(r, "collectionArtistName").unwrap_or_default(),
                year: s(r, "releaseDate").and_then(|d| d.get(0..4)?.parse::<u16>().ok()),
                genre: s(r, "primaryGenreName"),
                track_no: n(r, "trackNumber"),
                track_total: n(r, "trackCount"),
                disc_no: n(r, "discNumber"),
            });
        }
        Some(())
    })();
}

/// Deezer track search — title / artist / album / track + disc position.
pub(crate) fn deezer(agent: &ureq::Agent, query: &str, out: &mut Vec<TagCandidate>) {
    let _ = (|| -> Option<()> {
        let v: Value = agent
            .get("https://api.deezer.com/search/track")
            .header("User-Agent", UA)
            .query("q", query)
            .query("limit", "10")
            .call()
            .ok()?
            .body_mut()
            .read_json()
            .ok()?;
        let n = |r: &Value, k: &str| r.get(k).and_then(Value::as_u64).map(|x| x as u16);
        for r in v.get("data")?.as_array()? {
            let title = r.get("title").and_then(Value::as_str).unwrap_or("");
            if title.is_empty() {
                continue;
            }
            out.push(TagCandidate {
                source: "Deezer",
                title: title.to_string(),
                artist: r
                    .get("artist")
                    .and_then(|a| a.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                album: r
                    .get("album")
                    .and_then(|a| a.get("title"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                album_artist: String::new(),
                year: None,
                genre: None,
                track_no: n(r, "track_position"),
                track_total: None,
                disc_no: n(r, "disk_number"),
            });
        }
        Some(())
    })();
}

/// Max album editions fetched per source (e.g. regular + deluxe).
const MAX_EDITIONS: usize = 3;

/// Fetch whole albums from iTunes: pick the editions (exact artist) closest to
/// the local track count, then look up each one's full tracklist.
pub(crate) fn itunes_album(
    agent: &ureq::Agent,
    artist: &str,
    album: &str,
    count: usize,
    out: &mut Vec<AlbumMatch>,
) {
    let _ = (|| -> Option<()> {
        let v: Value = agent
            .get("https://itunes.apple.com/search")
            .header("User-Agent", UA)
            .query("term", format!("{artist} {album}"))
            .query("entity", "album")
            .query("limit", "20")
            .call()
            .ok()?
            .body_mut()
            .read_json()
            .ok()?;
        // candidate editions: (collectionId, name, artist, trackCount, title_sim, title_matched)
        let mut cands: Vec<(i64, String, String, usize, f32, bool)> = Vec::new();
        for r in v.get("results")?.as_array()? {
            let cid = match r.get("collectionId").and_then(Value::as_i64) {
                Some(c) => c,
                None => continue,
            };
            let cname = r
                .get("collectionName")
                .and_then(Value::as_str)
                .unwrap_or("");
            let aname = r.get("artistName").and_then(Value::as_str).unwrap_or("");
            // require the artist; the album title is preferred (below), not required —
            // so a romanized tag still finds an Arabic-titled release of the same artist.
            if !artist.trim().is_empty() && !artist_match(aname, artist) {
                continue;
            }
            if cands.iter().any(|c| c.0 == cid) {
                continue;
            }
            let tc = r.get("trackCount").and_then(Value::as_u64).unwrap_or(0) as usize;
            cands.push((
                cid,
                cname.to_string(),
                aname.to_string(),
                tc,
                title_sim(cname, album),
                album_matches(cname, album),
            ));
        }
        // if any edition's title actually matches, keep only those; otherwise fall
        // back to all of the artist's albums (cross-script titles, etc.).
        if cands.iter().any(|c| c.5) {
            cands.retain(|c| c.5);
        }
        // closest track count first (deluxe vs regular), then best title
        cands.sort_by(|a, b| {
            let (da, db) = (a.3.abs_diff(count), b.3.abs_diff(count));
            if count > 0 && da != db {
                da.cmp(&db)
            } else {
                b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal)
            }
        });
        for (cid, cname, aname, _, _, _) in cands.into_iter().take(MAX_EDITIONS) {
            let v2: Value = match agent
                .get("https://itunes.apple.com/lookup")
                .header("User-Agent", UA)
                .query("id", cid.to_string())
                .query("entity", "song")
                .call()
                .ok()
                .and_then(|mut r| r.body_mut().read_json().ok())
            {
                Some(v) => v,
                None => continue,
            };
            let s = |r: &Value, k: &str| r.get(k).and_then(Value::as_str).map(str::to_string);
            let n = |r: &Value, k: &str| r.get(k).and_then(Value::as_u64).map(|x| x as u16);
            let mut tracks = Vec::new();
            for r in v2
                .get("results")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if r.get("wrapperType").and_then(Value::as_str) != Some("track") {
                    continue;
                }
                let title = s(r, "trackName").unwrap_or_default();
                if title.is_empty() {
                    continue;
                }
                tracks.push(TagCandidate {
                    source: "iTunes",
                    title,
                    artist: s(r, "artistName").unwrap_or_default(),
                    album: cname.clone(),
                    album_artist: aname.clone(),
                    year: s(r, "releaseDate").and_then(|d| d.get(0..4)?.parse().ok()),
                    genre: s(r, "primaryGenreName"),
                    track_no: n(r, "trackNumber"),
                    track_total: n(r, "trackCount"),
                    disc_no: n(r, "discNumber"),
                });
            }
            if !tracks.is_empty() {
                tracks.sort_by_key(|t| (t.disc_no.unwrap_or(1), t.track_no.unwrap_or(0)));
                out.push(AlbumMatch {
                    source: "iTunes",
                    album: cname,
                    artist: aname,
                    tracks,
                });
            }
        }
        Some(())
    })();
}

/// Fetch whole albums from Deezer: the editions (exact artist) closest to the
/// local track count, then look up each one's tracks.
pub(crate) fn deezer_album(
    agent: &ureq::Agent,
    artist: &str,
    album: &str,
    count: usize,
    out: &mut Vec<AlbumMatch>,
) {
    let _ = (|| -> Option<()> {
        let v: Value = agent
            .get("https://api.deezer.com/search/album")
            .header("User-Agent", UA)
            .query("q", format!("{artist} {album}"))
            .query("limit", "20")
            .call()
            .ok()?
            .body_mut()
            .read_json()
            .ok()?;
        let mut cands: Vec<(i64, String, String, usize, f32, bool)> = Vec::new();
        for r in v.get("data")?.as_array()? {
            let id = match r.get("id").and_then(Value::as_i64) {
                Some(i) => i,
                None => continue,
            };
            let atitle = r.get("title").and_then(Value::as_str).unwrap_or("");
            let aname = r
                .get("artist")
                .and_then(|a| a.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if !artist.trim().is_empty() && !artist_match(aname, artist) {
                continue;
            }
            if cands.iter().any(|c| c.0 == id) {
                continue;
            }
            let nb = r.get("nb_tracks").and_then(Value::as_u64).unwrap_or(0) as usize;
            cands.push((
                id,
                atitle.to_string(),
                aname.to_string(),
                nb,
                title_sim(atitle, album),
                album_matches(atitle, album),
            ));
        }
        if cands.iter().any(|c| c.5) {
            cands.retain(|c| c.5);
        }
        cands.sort_by(|a, b| {
            let (da, db) = (a.3.abs_diff(count), b.3.abs_diff(count));
            if count > 0 && da != db {
                da.cmp(&db)
            } else {
                b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal)
            }
        });
        for (id, atitle, aname, _, _, _) in cands.into_iter().take(MAX_EDITIONS) {
            deezer_album_tracks(agent, id, &atitle, &aname, out);
        }
        Some(())
    })();
}

/// Look up one Deezer album's tracks → an `AlbumMatch`.
pub(crate) fn deezer_album_tracks(
    agent: &ureq::Agent,
    id: i64,
    atitle: &str,
    aname: &str,
    out: &mut Vec<AlbumMatch>,
) {
    let _ = (|| -> Option<()> {
        let v2: Value = agent
            .get(&format!("https://api.deezer.com/album/{id}"))
            .header("User-Agent", UA)
            .call()
            .ok()?
            .body_mut()
            .read_json()
            .ok()?;
        let year = v2
            .get("release_date")
            .and_then(Value::as_str)
            .and_then(|d| d.get(0..4)?.parse().ok());
        let genre = v2
            .get("genres")
            .and_then(|g| g.get("data"))
            .and_then(|d| d.get(0))
            .and_then(|g| g.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let n = |r: &Value, k: &str| r.get(k).and_then(Value::as_u64).map(|x| x as u16);
        let mut tracks = Vec::new();
        for r in v2.get("tracks").and_then(|t| t.get("data"))?.as_array()? {
            let title = r.get("title").and_then(Value::as_str).unwrap_or("");
            if title.is_empty() {
                continue;
            }
            tracks.push(TagCandidate {
                source: "Deezer",
                title: title.to_string(),
                artist: r
                    .get("artist")
                    .and_then(|a| a.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                album: atitle.to_string(),
                album_artist: aname.to_string(),
                year,
                genre: genre.clone(),
                track_no: n(r, "track_position"),
                track_total: None,
                disc_no: n(r, "disk_number"),
            });
        }
        if !tracks.is_empty() {
            tracks.sort_by_key(|t| (t.disc_no.unwrap_or(1), t.track_no.unwrap_or(0)));
            out.push(AlbumMatch {
                source: "Deezer",
                album: atitle.to_string(),
                artist: aname.to_string(),
                tracks,
            });
        }
        Some(())
    })();
}

/// Fetch whole albums from **MusicBrainz** — its release database carries the
/// Deluxe / regional editions that iTunes and Deezer often lack. Picks the
/// editions (exact artist, same core name) closest to the local track count.
/// MusicBrainz asks for ~1 req/sec, so lookups are throttled.
pub(crate) fn musicbrainz_album(
    agent: &ureq::Agent,
    artist: &str,
    album: &str,
    count: usize,
    out: &mut Vec<AlbumMatch>,
) {
    let _ = (|| -> Option<()> {
        let query = format!("artist:\"{artist}\" AND release:\"{album}\"");
        let v: Value = agent
            .get("https://musicbrainz.org/ws/2/release/")
            .header("User-Agent", UA)
            .query("query", &query)
            .query("fmt", "json")
            .query("limit", "25")
            .call()
            .ok()?
            .body_mut()
            .read_json()
            .ok()?;
        // candidate editions: (mbid, title, artist, track_count, title_matched)
        let mut cands: Vec<(String, String, String, usize, bool)> = Vec::new();
        for r in v.get("releases")?.as_array()? {
            let id = match r.get("id").and_then(Value::as_str) {
                Some(i) => i,
                None => continue,
            };
            let title = r.get("title").and_then(Value::as_str).unwrap_or("");
            let aname = r
                .get("artist-credit")
                .and_then(|a| a.get(0))
                .and_then(|a| a.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if !artist.trim().is_empty() && !artist_match(aname, artist) {
                continue;
            }
            let tc = r.get("track-count").and_then(Value::as_u64).unwrap_or(0) as usize;
            if cands.iter().any(|c| c.0 == id) {
                continue;
            }
            cands.push((
                id.to_string(),
                title.to_string(),
                aname.to_string(),
                tc,
                album_matches(title, album),
            ));
        }
        if cands.iter().any(|c| c.4) {
            cands.retain(|c| c.4);
        }
        if count > 0 {
            cands.sort_by_key(|c| c.3.abs_diff(count));
        }
        for (id, title, aname, _, _) in cands.into_iter().take(MAX_EDITIONS) {
            // throttle to respect MusicBrainz's ~1 req/sec guideline
            std::thread::sleep(Duration::from_millis(1100));
            let v2: Value = match agent
                .get(&format!("https://musicbrainz.org/ws/2/release/{id}"))
                .header("User-Agent", UA)
                .query("inc", "recordings")
                .query("fmt", "json")
                .call()
                .ok()
                .and_then(|mut r| r.body_mut().read_json().ok())
            {
                Some(v) => v,
                None => continue,
            };
            let year = v2
                .get("date")
                .and_then(Value::as_str)
                .and_then(|d| d.get(0..4)?.parse().ok());
            let media = match v2.get("media").and_then(Value::as_array) {
                Some(m) => m,
                None => continue,
            };
            let total: usize = media
                .iter()
                .filter_map(|m| m.get("tracks").and_then(Value::as_array))
                .map(|t| t.len())
                .sum();
            let mut tracks = Vec::new();
            for (di, m) in media.iter().enumerate() {
                for t in m
                    .get("tracks")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let ttitle = t.get("title").and_then(Value::as_str).unwrap_or("");
                    if ttitle.is_empty() {
                        continue;
                    }
                    tracks.push(TagCandidate {
                        source: "MusicBrainz",
                        title: ttitle.to_string(),
                        artist: aname.clone(),
                        album: title.clone(),
                        album_artist: aname.clone(),
                        year,
                        genre: None,
                        track_no: t.get("position").and_then(Value::as_u64).map(|x| x as u16),
                        track_total: Some(total as u16),
                        disc_no: Some((di + 1) as u16),
                    });
                }
            }
            if !tracks.is_empty() {
                tracks.sort_by_key(|t| (t.disc_no.unwrap_or(1), t.track_no.unwrap_or(0)));
                out.push(AlbumMatch {
                    source: "MusicBrainz",
                    album: title,
                    artist: aname,
                    tracks,
                });
            }
        }
        Some(())
    })();
}
