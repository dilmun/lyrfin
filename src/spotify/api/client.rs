//! Spotify Web API worker + JSON parsers, extracted from `api`: the request
//! loop (`spawn`) that hand-rolls library/search/browse/save calls over ureq +
//! serde_json (coalescing rapid searches latest-wins, backing off on 429s) and
//! the response→`Item` parsers. All the metadata the UI shows comes from here.

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use serde_json::Value;

use super::*;

/// First string from a JSON field (string or number).
fn s(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        // trim edge whitespace: some Spotify titles/names carry a stray leading or
        // trailing space that would otherwise misalign a column or list row.
        .map(|x| x.trim().to_string())
        .unwrap_or_default()
}

/// Join an array of artist objects into "A, B".
fn artist_names(v: &Value) -> String {
    v.get("artists")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.get("name").and_then(|n| n.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

/// `spotify:artist:…` of the first listed artist, if any (for the artist pane).
fn first_artist_uri(v: &Value) -> Option<String> {
    v.get("artists")?
        .as_array()?
        .first()?
        .get("uri")?
        .as_str()
        .map(str::to_string)
}

/// Image URL whose width is closest to ~300px — big enough that the playback bar
/// (and any thumbnail) renders crisply when scaled down to fit the cell area,
/// without pulling the full 640px artwork. `Resize::Fit` never upscales, so a
/// too-small source would render tiny; this keeps the art sharp.
fn pick_image(images: Option<&Value>) -> Option<String> {
    const TARGET: i64 = 300;
    let arr = images?.as_array()?;
    if arr.is_empty() {
        return None;
    }
    let mut best: Option<(&Value, i64)> = None;
    for img in arr {
        let w = img.get("width").and_then(|x| x.as_i64()).unwrap_or(TARGET);
        let score = (w - TARGET).abs();
        if best.map(|(_, bs)| score < bs).unwrap_or(true) {
            best = Some((img, score));
        }
    }
    best.or_else(|| arr.first().map(|i| (i, 0)))
        .and_then(|(i, _)| i.get("url").and_then(|u| u.as_str()).map(str::to_string))
}

/// The widest available image — for the artist hero photo, which fills the pane
/// width and would look soft if upscaled from the ~300px thumbnail.
fn pick_image_largest(images: Option<&Value>) -> Option<String> {
    let arr = images?.as_array()?;
    arr.iter()
        .max_by_key(|img| img.get("width").and_then(|x| x.as_i64()).unwrap_or(0))
        .or_else(|| arr.first())
        .and_then(|i| i.get("url").and_then(|u| u.as_str()).map(str::to_string))
}

/// Year from a Spotify `release_date` ("2023" / "2023-05" / "2023-05-01").
fn parse_year(release_date: &str) -> Option<u16> {
    release_date.split('-').next()?.parse().ok()
}

fn track_item(t: &Value) -> Item {
    let album = t.get("album");
    Item {
        uri: s(t, "uri"),
        name: s(t, "name"),
        subtitle: artist_names(t),
        album: album.map(|a| s(a, "name")).unwrap_or_default(),
        image: pick_image(album.and_then(|a| a.get("images"))),
        kind: Kind::Track,
        duration_ms: t.get("duration_ms").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        artist_uri: first_artist_uri(t),
        year: album
            .map(|a| s(a, "release_date"))
            .as_deref()
            .and_then(parse_year),
        ..Default::default()
    }
}

/// Like [`track_item`] but for album tracks, which carry no `album` object — the
/// cover, album name, AND release year are inherited from the parent album.
fn track_item_with_image(
    t: &Value,
    image: Option<String>,
    album: String,
    year: Option<u16>,
) -> Item {
    Item {
        image,
        album,
        year,
        ..track_item(t)
    }
}

/// The bare id from a `spotify:{kind}:{id}` (or open.spotify.com) URI.
pub(super) fn uri_id(uri: &str) -> &str {
    uri.rsplit([':', '/']).next().unwrap_or(uri)
}

fn album_item(a: &Value) -> Item {
    Item {
        uri: s(a, "uri"),
        name: s(a, "name"),
        subtitle: artist_names(a),
        image: pick_image(a.get("images")),
        kind: Kind::Album,
        year: parse_year(&s(a, "release_date")),
        ..Default::default()
    }
}

fn artist_item(a: &Value) -> Item {
    Item {
        uri: s(a, "uri"),
        name: s(a, "name"),
        subtitle: String::new(), // just the name shows; followers are optional
        image: pick_image(a.get("images")),
        kind: Kind::Artist,
        followers: a
            .get("followers")
            .and_then(|f| f.get("total"))
            .and_then(|t| t.as_u64()),
        ..Default::default()
    }
}

pub(super) fn playlist_item(p: &Value) -> Item {
    Item {
        uri: s(p, "uri"),
        name: s(p, "name"),
        subtitle: p
            .get("owner")
            .and_then(|o| o.get("display_name"))
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string(),
        image: pick_image(p.get("images")),
        kind: Kind::Playlist,
        count: p
            .get("tracks")
            .and_then(|t| t.get("total"))
            .and_then(|n| n.as_u64())
            .map(|n| n as u32),
        ..Default::default()
    }
}

/// A podcast show (a container → its episodes). Publisher as the subtitle.
fn show_item(sh: &Value) -> Item {
    Item {
        uri: s(sh, "uri"),
        name: s(sh, "name"),
        subtitle: s(sh, "publisher"),
        image: pick_image(sh.get("images")),
        kind: Kind::Show,
        ..Default::default()
    }
}

/// One podcast episode → a playable [`Item`] (treated as a `Track`). Falls back
/// to the show's cover when the episode has no art of its own. The show name is
/// stored in `album` — the playback path needs it to find the episode's public
/// RSS feed (librespot can't decrypt Spotify-hosted episode audio) — and the show
/// URI in `show_uri`, so the artist pane can open the show's page.
fn episode_item(e: &Value, show_name: &str, show_uri: &str, show_cover: &Option<String>) -> Item {
    Item {
        uri: s(e, "uri"),
        name: s(e, "name"),
        subtitle: s(e, "release_date"),
        album: show_name.to_string(),
        image: pick_image(e.get("images")).or_else(|| show_cover.clone()),
        kind: Kind::Track,
        duration_ms: e.get("duration_ms").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        show_uri: (!show_uri.is_empty()).then(|| show_uri.to_string()),
        ..Default::default()
    }
}

/// Fetch one artist's details for the pane: photo, genres, follower count.
/// `/v1/artists/{id}` works for dev-mode apps (unlike top-tracks/playlist-tracks).
fn fetch_artist(
    agent: &ureq::Agent,
    token: &str,
    uri: &str,
) -> Result<(String, Option<String>, String, u64), Option<String>> {
    let v = get_json(agent, token, &format!("{API}/artists/{}", uri_id(uri)))?;
    let name = s(&v, "name");
    let image = pick_image_largest(v.get("images"));
    let genres = v
        .get("genres")
        .and_then(|g| g.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(" · ")
        })
        .unwrap_or_default();
    let followers = v
        .get("followers")
        .and_then(|f| f.get("total"))
        .and_then(|n| n.as_u64())
        .unwrap_or(0);
    Ok((name, image, genres, followers))
}

/// Fetch a podcast show's metadata for the pane's "About": publisher + description.
/// `/v1/shows/{id}` works for dev-mode apps (unlike podcast browse endpoints).
fn fetch_show_meta(
    agent: &ureq::Agent,
    token: &str,
    uri: &str,
) -> Result<(String, String), Option<String>> {
    let v = get_json(agent, token, &format!("{API}/shows/{}", uri_id(uri)))?;
    Ok((s(&v, "publisher"), s(&v, "description")))
}

/// Compact a follower count: 1820 → "1.8k", 2_500_000 → "2.5M".
pub fn fmt_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// GET a Web API path with the bearer token, retrying 429s (capped). `Err(None)`
/// signals 401 (unauthorized); `Err(Some(msg))` is any other failure.
/// Shown when Spotify returns 403 "not registered for this application": the
/// signed-in account isn't on the dev app's user allowlist (each dev app caps at
/// 25 users, and the client id is per-app). Actionable + honest — playback via
/// librespot is unaffected, only the Web API is gated for this account.
pub const NOT_REGISTERED_MSG: &str = "This account isn't registered on your Spotify app. Add it at developer.spotify.com/dashboard (your app → Users), or press 'c' to set this account's own client id. Playback still works.";

pub(super) fn get_json(
    agent: &ureq::Agent,
    token: &str,
    url: &str,
) -> Result<Value, Option<String>> {
    let mut attempt = 0;
    loop {
        let mut resp = match agent
            .get(url)
            .header("Authorization", &format!("Bearer {token}"))
            .call()
        {
            Ok(r) => r,
            // ureq 3 only errors here on transport/protocol issues (status is Ok now)
            Err(e) => return Err(Some(format!("can't reach Spotify ({e})"))),
        };
        match resp.status().as_u16() {
            200..=299 => return resp.body_mut().read_json().map_err(|e| Some(e.to_string())),
            401 => return Err(None),
            429 if attempt < 1 => {
                // one short retry; don't sit on a long Retry-After (the shared
                // client id rate-limits easily) — surface it quickly instead
                let wait = retry_after(&resp);
                std::thread::sleep(std::time::Duration::from_secs(wait));
                attempt += 1;
            }
            429 => {
                return Err(Some(
                    "rate-limited by Spotify (shared app id) — see 'use your own client id'".into(),
                ));
            }
            c => {
                let body = resp.body_mut().read_to_string().unwrap_or_default();
                let detail = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|v| v.get("error")?.get("message")?.as_str().map(str::to_string))
                    .unwrap_or(body);
                // 403 "user is not registered for this application" = the logged-in
                // account isn't on the dev app's allowlist (dev-mode apps cap at 25
                // users). Give an actionable message instead of the raw error; audio
                // still works (librespot uses its own client id), only the Web API is
                // blocked for this account.
                if c == 403 && detail.to_lowercase().contains("not registered") {
                    return Err(Some(NOT_REGISTERED_MSG.into()));
                }
                return Err(Some(format!("Spotify error {c}: {detail}")));
            }
        }
    }
}

/// Retry-After header (seconds) from a 429, clamped to a short 1–3s so a rate-limit
/// on the shared client id surfaces quickly instead of stalling the worker.
fn retry_after(resp: &ureq::http::Response<ureq::Body>) -> u64 {
    resp.headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1)
        .clamp(1, 3)
}

/// POST / PUT / DELETE a JSON body (empty `body` = no body, e.g. an unfollow),
/// sharing `get_json`'s 401/429/403 handling. Returns the parsed response, or
/// `Value::Null` when the endpoint answers with an empty body (a 200/201 with no
/// JSON). The only Web API writes besides Liked Songs go through here.
pub(super) fn send_json(
    agent: &ureq::Agent,
    token: &str,
    method: &str,
    url: &str,
    body: &str,
) -> Result<Value, Option<String>> {
    let mut attempt = 0;
    loop {
        // ureq 3 has no dynamic-method request builder; build an http::Request (the
        // body is an empty String for a no-body write, e.g. an unfollow) and run it.
        let request = match ureq::http::Request::builder()
            .method(method)
            .uri(url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(body.to_string())
        {
            Ok(r) => r,
            Err(e) => return Err(Some(e.to_string())),
        };
        let mut resp = match agent.run(request) {
            Ok(r) => r,
            Err(e) => return Err(Some(format!("can't reach Spotify ({e})"))),
        };
        match resp.status().as_u16() {
            // a create returns the playlist JSON; add/remove/rename/unfollow may
            // answer with an empty body → treat a non-JSON body as `Null`.
            200..=299 => return Ok(resp.body_mut().read_json().unwrap_or(Value::Null)),
            401 => return Err(None),
            429 if attempt < 1 => {
                let wait = retry_after(&resp);
                std::thread::sleep(std::time::Duration::from_secs(wait));
                attempt += 1;
            }
            c => {
                let raw = resp.body_mut().read_to_string().unwrap_or_default();
                let detail = serde_json::from_str::<Value>(&raw)
                    .ok()
                    .and_then(|v| v.get("error")?.get("message")?.as_str().map(str::to_string))
                    .filter(|m| !m.is_empty())
                    .unwrap_or(raw);
                // Surface Spotify's OWN reason (its `error.message`) verbatim — a 403 on
                // a write can be Development-mode quota, an account restriction, or a
                // real scope gap, and only the raw message distinguishes them.
                return Err(Some(format!("Spotify error {c}: {detail}")));
            }
        }
    }
}

/// Extract `items[]` (optionally under a wrapper key) and map each via `f`,
/// unwrapping a per-item `.track`/`.album` envelope when present.
fn map_items(
    v: &Value,
    wrapper: Option<&str>,
    envelope: Option<&str>,
    f: impl Fn(&Value) -> Item,
) -> Vec<Item> {
    let base = match wrapper {
        Some(w) => v.get(w),
        None => Some(v),
    };
    base.and_then(|b| b.get("items"))
        .and_then(|i| i.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|it| {
                    let obj = match envelope {
                        Some(e) => it.get(e)?,
                        None => it,
                    };
                    (!obj.is_null()).then(|| f(obj))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn fetch_library(
    agent: &ureq::Agent,
    token: &str,
    section: Section,
) -> Result<Vec<Item>, Option<String>> {
    let items = match section {
        // Home + Browse are served by pathfinder GraphQL over librespot (see
        // `spotify::pathfinder`), never routed through this Web API worker.
        Section::Home | Section::Browse => Vec::new(),
        Section::LikedSongs => {
            let v = get_json(agent, token, &format!("{API}/me/tracks?limit=50"))?;
            map_items(&v, None, Some("track"), track_item)
        }
        Section::Playlists => {
            let v = get_json(agent, token, &format!("{API}/me/playlists?limit=50"))?;
            map_items(&v, None, None, playlist_item)
        }
        Section::Albums => {
            let v = get_json(agent, token, &format!("{API}/me/albums?limit=50"))?;
            map_items(&v, None, Some("album"), album_item)
        }
        Section::Artists => {
            let v = get_json(
                agent,
                token,
                &format!("{API}/me/following?type=artist&limit=50"),
            )?;
            map_items(&v, Some("artists"), None, artist_item)
        }
        Section::Podcasts => {
            let v = get_json(agent, token, &format!("{API}/me/shows?limit=50"))?;
            map_items(&v, None, Some("show"), show_item)
        }
        Section::RecentlyPlayed => {
            let v = get_json(
                agent,
                token,
                &format!("{API}/me/player/recently-played?limit=50"),
            )?;
            map_items(&v, None, Some("track"), track_item)
        }
        Section::TopTracks => {
            let v = get_json(agent, token, &format!("{API}/me/top/tracks?limit=50"))?;
            // top tracks come straight in `items[]` (no per-item envelope)
            map_items(&v, None, None, track_item)
        }
    };
    Ok(items)
}

/// Fetch the playable tracks of a container: an album's track list, a playlist's
/// tracks, or an artist's top tracks.
/// How many of a show's newest episodes to page in when it's opened. A cap keeps a
/// huge back-catalogue (JRE has ~2500 episodes) from fanning out into dozens of
/// sequential requests; 200 (four pages) covers realistic browsing of recent
/// episodes while bounding the drill-in latency.
const MAX_SHOW_EPISODES: usize = 200;

fn fetch_open(
    agent: &ureq::Agent,
    token: &str,
    uri: &str,
    kind: Kind,
) -> Result<Vec<Item>, Option<String>> {
    let id = uri_id(uri);
    let items = match kind {
        Kind::Album => {
            // one call gives the cover + the (cover-less) track list
            let v = get_json(agent, token, &format!("{API}/albums/{id}"))?;
            let cover = pick_image(v.get("images"));
            let album_name = s(&v, "name");
            // the per-track JSON has no album object → inherit the album's year too
            let year = parse_year(&s(&v, "release_date"));
            v.get("tracks")
                .and_then(|t| t.get("items"))
                .and_then(|i| i.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|t| track_item_with_image(t, cover.clone(), album_name.clone(), year))
                        .collect()
                })
                .unwrap_or_default()
        }
        Kind::Playlist => {
            // Spotify blocks /playlists/{id}/tracks for development-mode apps
            // (403). There's no Web API fallback — translate to an honest note.
            match get_json(
                agent,
                token,
                &format!("{API}/playlists/{id}/tracks?limit=50"),
            ) {
                Ok(v) => map_items(&v, None, Some("track"), track_item),
                Err(Some(m)) if m.contains("403") => {
                    return Err(Some(
                        "Spotify's API blocks playlist tracks for personal apps. Open albums, liked songs, or artists instead.".into(),
                    ));
                }
                Err(e) => return Err(e),
            }
        }
        Kind::Artist => {
            // top-tracks is 403 for dev-mode apps; show the discography instead
            // (album/single → drill into one for its tracks). limit caps at 10.
            let v = get_json(
                agent,
                token,
                &format!("{API}/artists/{id}/albums?include_groups=album,single&limit=10"),
            )?;
            map_items(&v, None, None, album_item)
        }
        Kind::Show => {
            // /shows/{id} gives the show cover + name + uri (its embedded episode list
            // tops out at 50).
            let v = get_json(agent, token, &format!("{API}/shows/{id}"))?;
            let cover = pick_image(v.get("images"));
            let show_name = s(&v, "name");
            let show_uri = s(&v, "uri");
            // Page the episodes from the dedicated endpoint (consistent market
            // filtering) up to a cap — a long-running show has thousands, so grab the
            // newest history without an unbounded fan-out. `offset` counts raw items
            // (Spotify's offset space); `next` being null marks the last page.
            let mut items: Vec<Item> = Vec::new();
            let mut offset = 0usize;
            loop {
                let page = get_json(
                    agent,
                    token,
                    &format!(
                        "{API}/shows/{id}/episodes?limit=50&offset={offset}&market=from_token"
                    ),
                )?;
                let Some(arr) = page.get("items").and_then(Value::as_array) else {
                    break;
                };
                if arr.is_empty() {
                    break;
                }
                offset += arr.len();
                items.extend(
                    arr.iter()
                        .filter(|e| !e.is_null())
                        .map(|e| episode_item(e, &show_name, &show_uri, &cover)),
                );
                let has_next = page.get("next").map(|n| !n.is_null()).unwrap_or(false);
                if !has_next || items.len() >= MAX_SHOW_EPISODES {
                    break;
                }
            }
            items
        }
        // a track/episode has no contents to open; a browse category never routes
        // through the Web API (it opens via pathfinder over the session)
        Kind::Track | Kind::Category => Vec::new(),
    };
    Ok(items)
}

/// Is `uri` in the user's Liked Songs? (GET /me/tracks/contains)
fn check_saved(agent: &ureq::Agent, token: &str, uri: &str) -> Result<bool, Option<String>> {
    let id = uri_id(uri);
    let v = get_json(agent, token, &format!("{API}/me/tracks/contains?ids={id}"))?;
    Ok(v.as_array()
        .and_then(|a| a.first())
        .and_then(|b| b.as_bool())
        .unwrap_or(false))
}

/// Add (`saved`) or remove a track from Liked Songs (PUT/DELETE /me/tracks).
fn set_saved(
    agent: &ureq::Agent,
    token: &str,
    uri: &str,
    saved: bool,
) -> Result<(), Option<String>> {
    let id = uri_id(uri);
    let method = if saved { "PUT" } else { "DELETE" };
    // Send the id in a JSON body, NOT just `?ids=`: an empty-body PUT/DELETE carries
    // no Content-Length, which Spotify rejects with 411 Length Required. `send_string`
    // sets Content-Length; the body form is the documented way to pass `ids`.
    let body = format!("{{\"ids\":[\"{id}\"]}}");
    let request = ureq::http::Request::builder()
        .method(method)
        .uri(format!("{API}/me/tracks"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body(body)
        .map_err(|e| Some(e.to_string()))?;
    let resp = agent.run(request).map_err(|e| Some(e.to_string()))?;
    match resp.status().as_u16() {
        200..=299 => Ok(()),
        401 => Err(None),
        // 403 = the app refused the change. With a personal client id this usually
        // means the account isn't allow-listed on the app (dev-mode 25-user cap).
        403 => Err(Some(
            "403 — add your account at developer.spotify.com → your app → Users".into(),
        )),
        c => Err(Some(format!("Spotify error {c}"))),
    }
}

/// Is the show saved / the artist followed? (`GET /me/shows/contains` or
/// `/me/following/contains`). `Kind` other than Show/Artist is never followable.
fn check_follow(
    agent: &ureq::Agent,
    token: &str,
    uri: &str,
    kind: Kind,
) -> Result<bool, Option<String>> {
    let id = uri_id(uri);
    let url = match kind {
        Kind::Show => format!("{API}/me/shows/contains?ids={id}"),
        Kind::Artist => format!("{API}/me/following/contains?type=artist&ids={id}"),
        _ => return Ok(false),
    };
    let v = get_json(agent, token, &url)?;
    Ok(v.as_array()
        .and_then(|a| a.first())
        .and_then(Value::as_bool)
        .unwrap_or(false))
}

/// Follow (`follow`) or unfollow a show (`PUT/DELETE /me/shows`) or artist
/// (`.../me/following?type=artist`). The id rides in BOTH the query (required for
/// shows) and a JSON body (so the PUT/DELETE carries a Content-Length and avoids
/// `411 Length Required`, mirroring `set_saved`).
fn set_follow(
    agent: &ureq::Agent,
    token: &str,
    uri: &str,
    kind: Kind,
    follow: bool,
) -> Result<(), Option<String>> {
    let id = uri_id(uri);
    let method = if follow { "PUT" } else { "DELETE" };
    let url = match kind {
        Kind::Show => format!("{API}/me/shows?ids={id}"),
        Kind::Artist => format!("{API}/me/following?type=artist&ids={id}"),
        _ => return Ok(()),
    };
    let body = format!("{{\"ids\":[\"{id}\"]}}");
    send_json(agent, token, method, &url, &body).map(|_| ())
}

/// A podcast episode's parent show URI (`GET /episodes/{id}` → `show.uri`). Used to
/// open the show from an episode that carries no `show_uri` of its own.
fn resolve_show_uri(
    agent: &ureq::Agent,
    token: &str,
    episode_uri: &str,
) -> Result<Option<String>, Option<String>> {
    let id = uri_id(episode_uri);
    let v = get_json(
        agent,
        token,
        &format!("{API}/episodes/{id}?market=from_token"),
    )?;
    Ok(v.get("show")
        .and_then(|s| s.get("uri"))
        .and_then(Value::as_str)
        .map(str::to_string))
}

/// Toggle a show/artist follow: read the current state, flip it, return the new one.
fn toggle_follow(
    agent: &ureq::Agent,
    token: &str,
    uri: &str,
    kind: Kind,
) -> Result<bool, Option<String>> {
    let want = !check_follow(agent, token, uri, kind)?;
    set_follow(agent, token, uri, kind, want)?;
    Ok(want)
}

fn fetch_search(agent: &ureq::Agent, token: &str, query: &str) -> Result<SpResult, Option<String>> {
    let q = crate::spotify::api::enc(query.trim());
    // Spotify caps the search limit at 10 for non-extended apps (limit>10 → 400
    // "Invalid limit"); library endpoints still allow 50. `market=from_token`
    // applies the account's country — required so show results aren't a null array,
    // and so only playable items come back (podcasts return empty in markets that
    // don't license them, e.g. IQ — see the Podcasts empty-state message).
    let url = format!(
        "{API}/search?q={q}&type=track,album,artist,playlist,show&limit=10&market=from_token"
    );
    let v = get_json(agent, token, &url)?;
    Ok(SpResult::Search {
        key: String::new(), // filled by caller
        tracks: map_items(
            v.get("tracks").unwrap_or(&Value::Null),
            None,
            None,
            track_item,
        ),
        albums: map_items(
            v.get("albums").unwrap_or(&Value::Null),
            None,
            None,
            album_item,
        ),
        artists: map_items(
            v.get("artists").unwrap_or(&Value::Null),
            None,
            None,
            artist_item,
        ),
        playlists: map_items(
            v.get("playlists").unwrap_or(&Value::Null),
            None,
            None,
            playlist_item,
        ),
        shows: map_items(
            v.get("shows").unwrap_or(&Value::Null),
            None,
            None,
            show_item,
        ),
    })
}

/// Percent-encode a query value.
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Spawn the Web API worker. Coalesces: only the latest search runs; library
/// loads are all processed.
pub fn spawn() -> (Sender<SpRequest>, Receiver<SpResult>) {
    let (req_tx, req_rx) = unbounded::<SpRequest>();
    let (res_tx, res_rx) = unbounded::<SpResult>();
    std::thread::Builder::new()
        .name("lyrfin-spotify-api".into())
        .spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_connect(Some(std::time::Duration::from_secs(8)))
                .timeout_recv_body(Some(std::time::Duration::from_secs(15)))
                // keep 4xx/5xx as Ok(response) so the retry/rate-limit blocks below can
                // read the status, Retry-After header, and error body (ureq 3 otherwise
                // turns them into a response-less Error::StatusCode).
                .http_status_as_error(false)
                .build()
                .into();
            let process = |req: SpRequest| {
                let msg = match req {
                    SpRequest::Library {
                        section,
                        token,
                        key,
                    } => match fetch_library(&agent, &token, section) {
                        Ok(items) => SpResult::Library { key, items },
                        Err(None) => SpResult::Unauthorized { key },
                        Err(Some(msg)) => SpResult::Error { key, msg },
                    },
                    SpRequest::Search { query, token, key } => {
                        match fetch_search(&agent, &token, &query) {
                            Ok(SpResult::Search {
                                tracks,
                                albums,
                                artists,
                                playlists,
                                shows,
                                ..
                            }) => SpResult::Search {
                                key,
                                tracks,
                                albums,
                                artists,
                                playlists,
                                shows,
                            },
                            Ok(other) => other,
                            Err(None) => SpResult::Unauthorized { key },
                            Err(Some(msg)) => SpResult::Error { key, msg },
                        }
                    }
                    SpRequest::Open {
                        uri,
                        kind,
                        token,
                        key,
                    } => match fetch_open(&agent, &token, &uri, kind) {
                        Ok(items) => SpResult::Opened { key, items },
                        Err(None) => SpResult::Unauthorized { key },
                        Err(Some(msg)) => SpResult::Error { key, msg },
                    },
                    SpRequest::CheckSaved { uri, token } => {
                        // a check never disturbs the view; default to "not saved"
                        let saved = check_saved(&agent, &token, &uri).unwrap_or(false);
                        SpResult::Saved { uri, saved }
                    }
                    SpRequest::SetSaved { uri, saved, token } => {
                        match set_saved(&agent, &token, &uri, saved) {
                            Ok(()) => SpResult::Saved { uri, saved },
                            Err(None) => SpResult::Unauthorized { key: String::new() },
                            Err(Some(msg)) => SpResult::Error {
                                key: String::new(),
                                msg: format!("Couldn't update Liked Songs: {msg}"),
                            },
                        }
                    }
                    SpRequest::Artist { uri, token, key } => {
                        match fetch_artist(&agent, &token, &uri) {
                            Ok((name, image, genres, followers)) => SpResult::Artist {
                                uri,
                                name,
                                image,
                                genres,
                                followers,
                            },
                            Err(None) => SpResult::Unauthorized { key },
                            Err(Some(msg)) => SpResult::Error { key, msg },
                        }
                    }
                    SpRequest::ShowMeta { uri, token } => {
                        match fetch_show_meta(&agent, &token, &uri) {
                            Ok((publisher, description)) => SpResult::ShowMeta {
                                uri,
                                publisher,
                                description,
                            },
                            Err(None) => SpResult::Unauthorized { key: String::new() },
                            Err(Some(msg)) => SpResult::Error {
                                key: String::new(),
                                msg: format!("Couldn't load show info: {msg}"),
                            },
                        }
                    }
                    SpRequest::ToggleFollow { uri, kind, token } => {
                        match toggle_follow(&agent, &token, &uri, kind) {
                            Ok(followed) => SpResult::Follow { uri, followed },
                            Err(None) => SpResult::Unauthorized { key: String::new() },
                            Err(Some(msg)) => SpResult::Error {
                                key: String::new(),
                                // Spotify gates the library-write endpoints (/me/shows,
                                // /me/following) for apps in Development mode — a bare 403
                                // that no scope/re-login fixes. Be honest and point to the
                                // read-backed workaround (follow in Spotify → syncs here).
                                msg: if msg.contains("403") {
                                    "Following isn't available here — this Spotify app is in \
                                     Development mode, which blocks library writes. Follow it in \
                                     the Spotify app instead and it'll show up in Your Shows here."
                                        .to_string()
                                } else {
                                    format!("Couldn't update follow: {msg}")
                                },
                            },
                        }
                    }
                    SpRequest::ResolveShow {
                        episode_uri,
                        name,
                        token,
                    } => match resolve_show_uri(&agent, &token, &episode_uri) {
                        Ok(uri) => SpResult::ShowResolved { uri, name },
                        Err(None) => SpResult::Unauthorized { key: String::new() },
                        Err(Some(msg)) => SpResult::Error {
                            key: String::new(),
                            msg,
                        },
                    },
                    // ---- playlist writes + the writable-playlist fetch ----
                    SpRequest::MyPlaylists {
                        token,
                        user_id,
                        key,
                    } => match super::playlist::fetch_my_playlists(&agent, &token, &user_id) {
                        Ok(items) => SpResult::MyPlaylists {
                            key,
                            items,
                            error: None,
                        },
                        Err(None) => SpResult::MyPlaylists {
                            key,
                            items: Vec::new(),
                            error: Some(super::playlist::AUTH_EXPIRED.into()),
                        },
                        Err(Some(msg)) => SpResult::MyPlaylists {
                            key,
                            items: Vec::new(),
                            error: Some(msg),
                        },
                    },
                    SpRequest::CreatePlaylist { token, name, uris } => {
                        super::playlist::write_result(
                            PlaylistOp::Create,
                            super::playlist::create_playlist(&agent, &token, &name, &uris),
                        )
                    }
                    SpRequest::AddToPlaylist {
                        token,
                        playlist_uri,
                        uris,
                        name,
                    } => super::playlist::write_result(
                        PlaylistOp::Add,
                        super::playlist::add_tracks(&agent, &token, &playlist_uri, &uris, &name),
                    ),
                    SpRequest::RenamePlaylist {
                        token,
                        playlist_uri,
                        name,
                    } => super::playlist::write_result(
                        PlaylistOp::Rename,
                        super::playlist::rename(&agent, &token, &playlist_uri, &name),
                    ),
                    SpRequest::ReplacePlaylistItems {
                        token,
                        playlist_uri,
                        uris,
                        name,
                    } => super::playlist::write_result(
                        PlaylistOp::Remove,
                        super::playlist::replace_items(&agent, &token, &playlist_uri, &uris, &name),
                    ),
                    SpRequest::UnfollowPlaylist {
                        token,
                        playlist_uri,
                        name,
                    } => super::playlist::write_result(
                        PlaylistOp::Unfollow,
                        super::playlist::unfollow(&agent, &token, &playlist_uri, &name),
                    ),
                };
                let _ = res_tx.send(msg);
            };
            while let Ok(first) = req_rx.recv() {
                let mut pending = vec![first];
                loop {
                    match req_rx.try_recv() {
                        Ok(r) => pending.push(r),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                // run every library load; only the newest search
                let last_search = pending
                    .iter()
                    .rposition(|r| matches!(r, SpRequest::Search { .. }));
                for (i, r) in pending.into_iter().enumerate() {
                    if matches!(r, SpRequest::Search { .. }) && Some(i) != last_search {
                        continue;
                    }
                    process(r);
                }
            }
        })
        .expect("spawn spotify-api thread");
    (req_tx, res_rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_track_with_artists_and_cover() {
        let j = serde_json::json!({
            "uri": "spotify:track:1",
            "name": "Song",
            "duration_ms": 200000,
            "artists": [{"name": "A"}, {"name": "B"}],
            "album": {"images": [
                {"url":"big","width":640},
                {"url":"mid","width":300},
                {"url":"small","width":64}
            ]}
        });
        let it = track_item(&j);
        assert_eq!(it.name, "Song");
        assert_eq!(it.subtitle, "A, B");
        assert_eq!(it.image.as_deref(), Some("mid")); // ~300px chosen (crisp, not tiny)
        assert_eq!(it.duration_ms, 200000);
        assert_eq!(it.kind, Kind::Track);
    }

    #[test]
    fn artist_hero_picks_the_widest_image() {
        // thumbnails target ~300px, but the artist hero fills the pane width, so it
        // takes the largest available (640) to avoid an upscaled, soft photo.
        let images = serde_json::json!([
            {"url":"mid","width":320},
            {"url":"big","width":640},
            {"url":"small","width":160},
        ]);
        assert_eq!(pick_image_largest(Some(&images)).as_deref(), Some("big"));
        assert_eq!(pick_image_largest(None), None);
    }

    #[test]
    fn map_items_unwraps_track_envelope() {
        let j = serde_json::json!({
            "items": [
                {"track": {"uri":"spotify:track:1","name":"X","artists":[{"name":"Y"}]}},
                {"track": serde_json::Value::Null}
            ]
        });
        let items = map_items(&j, None, Some("track"), track_item);
        assert_eq!(items.len(), 1); // the null track is skipped
        assert_eq!(items[0].name, "X");
    }

    #[test]
    fn fmt_count_compacts() {
        assert_eq!(fmt_count(500), "500");
        assert_eq!(fmt_count(1800), "1.8k");
        assert_eq!(fmt_count(2_500_000), "2.5M");
    }

    #[test]
    fn uri_id_extracts_bare_id() {
        assert_eq!(
            uri_id("spotify:album:4aawyAB9vmqN3uQ7FjRGTy"),
            "4aawyAB9vmqN3uQ7FjRGTy"
        );
        assert_eq!(uri_id("spotify:playlist:37i9dQ"), "37i9dQ");
        assert_eq!(uri_id("https://open.spotify.com/track/abc123"), "abc123");
        assert_eq!(uri_id("abc123"), "abc123");
    }

    #[test]
    fn album_track_inherits_cover() {
        // album tracks carry no `album` field; the parent cover is attached
        let t = serde_json::json!({
            "uri": "spotify:track:9",
            "name": "Cut",
            "artists": [{"name": "Band"}],
            "duration_ms": 123000
        });
        let it = track_item_with_image(
            &t,
            Some("cover.jpg".into()),
            "Greatest Hits".into(),
            Some(1998),
        );
        assert_eq!(it.name, "Cut");
        assert_eq!(it.subtitle, "Band");
        assert_eq!(it.album, "Greatest Hits");
        assert_eq!(it.image.as_deref(), Some("cover.jpg"));
        assert_eq!(
            it.year,
            Some(1998),
            "the album's year is inherited by its tracks"
        );
        assert_eq!(it.kind, Kind::Track);
    }
}
