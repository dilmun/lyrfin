//! Spotify's internal **pathfinder GraphQL** client (`api-partner.spotify.com`),
//! reached over librespot's authenticated `http_client` with the session's login5
//! token + client-token. This is how lyrfin pulls browse/home catalog data the
//! public Web API no longer serves to dev-mode apps (see `browse-all-feasibility`).
//!
//! It is an **unofficial, undocumented** API: the persisted-query hashes below
//! rotate, so every call degrades gracefully (returns `Err`, never panics) and the
//! hashes live in one place to refresh. Keep this module the single point of change.

use std::collections::HashSet;

use http_body_util::BodyExt;
use librespot::core::session::Session;
use serde_json::Value;

use crate::spotify::api::{Item, Kind};

const PATHFINDER: &str = "https://api-partner.spotify.com/pathfinder/v1/query";

/// Persisted-query hashes (verified 2026-07; rotate over time — refresh here).
const HOME_HASH: &str = "a68635e823cd71d9f6810ec221d339348371ef0b878ec6b846fc36b234219c59";
const BROWSE_HASH: &str = "177a4ae12a90e35d335f060216ce5df7864a228c6ca262bd5ed90b37c2419dd9";
/// The "Explore all categories" root browse page (the categories grid) — the app
/// opens the Browse section by drilling into this uri.
pub(crate) const BROWSE_ROOT: &str = "spotify:page:0JQ5DArNBzkmxXHCqFLx3c";
/// The **podcast** browse page — Spotify's editorial "Top Podcasts" + trending
/// shelves and podcast category tiles. A sibling of [`BROWSE_ROOT`] under the same
/// `browsePage` operation (only the page uri differs), so it drills in via the very
/// same `fetch_browse_page` path — the Podcasts hub reuses the music-browse machinery.
pub(crate) const PODCASTS_BROWSE_ROOT: &str = "spotify:page:0JQ5DArNBzkmxXHCqFLx2J";
/// Canonical name given to the overflow "browse all categories" card so every hub —
/// music, podcasts — surfaces it as one full-width **button** (its own row) rather
/// than a tile in the category grid. Clicking it opens the full categories page.
/// Spotify labels this card differently per hub ("See all categories", "Browse all",
/// "Explore all categories"); [`is_browse_all_overflow`] folds them all to this name.
pub(crate) const ALL_CATEGORIES_LABEL: &str = "Browse all categories";

/// Whether a category-card label is Spotify's overflow "open the full categories
/// page" link, which every hub words slightly differently ("See all categories",
/// "Explore all categories", "Browse all", "View all"…). Matching the verb prefix —
/// rather than one exact string — keeps the standardised button working across hubs
/// without over-matching a real genre (none of which start with these prefixes).
fn is_browse_all_overflow(label: &str) -> bool {
    let low = label.trim().to_ascii_lowercase();
    [
        "see all",
        "browse all",
        "explore all",
        "view all",
        "show all",
    ]
    .iter()
    .any(|prefix| low.starts_with(prefix))
}

/// The Spotify **home** feed (the shelves of playlists/albums the app opens with),
/// flattened + de-duplicated into a browseable list. Errors (rotated hash, schema
/// change, offline) surface as `Err(msg)` for the pane to show.
pub async fn fetch_home(session: &Session) -> Result<Vec<Item>, String> {
    let country = session.country();
    let variables = format!(
        r#"{{"timeZone":"UTC","sp_t":"","country":"{country}","facet":null,"sectionItemsLimit":20}}"#
    );
    let json = query(session, "home", HOME_HASH, &variables).await?;
    Ok(parse_home(&json))
}

/// A browse page: the categories root (→ `Kind::Category` tiles), or one category's
/// page (→ its playlist shelves, section-tagged). Same operation, different content.
pub async fn fetch_browse_page(
    session: &Session,
    uri: &str,
    limit: usize,
) -> Result<Vec<Item>, String> {
    // `sectionPagination` bounds items-per-shelf: `limit` starts at 50 (fills a flat
    // grid generously) and is grown on scroll to page a Podcast Charts / Categories
    // grid in, reusing this one known-good query rather than a separate op.
    let variables = format!(
        r#"{{"uri":"{uri}","pagePagination":{{"offset":0,"limit":40}},"sectionPagination":{{"offset":0,"limit":{limit}}}}}"#
    );
    let json = query(session, "browsePage", BROWSE_HASH, &variables).await?;
    Ok(parse_browse(&json))
}

/// Fire one persisted GraphQL operation and return the parsed JSON (or an error,
/// including a GraphQL-level `errors` array — e.g. a rotated hash).
async fn query(session: &Session, op: &str, hash: &str, variables: &str) -> Result<Value, String> {
    let token = session
        .login5()
        .auth_token()
        .await
        .map_err(|e| format!("auth token: {e}"))?;
    let client_token = session
        .spclient()
        .client_token()
        .await
        .map_err(|e| format!("client token: {e}"))?;
    let ext = format!(r#"{{"persistedQuery":{{"version":1,"sha256Hash":"{hash}"}}}}"#);
    let url = format!(
        "{PATHFINDER}?operationName={op}&variables={}&extensions={}",
        pct(variables),
        pct(&ext)
    );
    let req = http::Request::builder()
        .method(http::Method::GET)
        .uri(&url)
        .header(http::header::ACCEPT, "application/json")
        .header("app-platform", "WebPlayer")
        .header("client-token", client_token)
        .header(
            http::header::AUTHORIZATION,
            format!("{} {}", token.token_type, token.access_token),
        )
        .body(bytes::Bytes::new())
        .map_err(|e| format!("build: {e}"))?;
    let fut = session
        .http_client()
        .request_fut(req)
        .map_err(|e| format!("send: {e}"))?;
    let resp = fut.await.map_err(|e| format!("net: {e}"))?;
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .map_err(|e| format!("body: {e}"))?
        .to_bytes();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let json: Value = serde_json::from_slice(&body).map_err(|e| format!("json: {e}"))?;
    if let Some(errors) = json.get("errors").filter(|e| !e.is_null()) {
        return Err(format!("graphql: {errors}"));
    }
    Ok(json)
}

/// Parse `data.home.sectionContainer.sections[]` into items, each tagged with its
/// shelf title (`Item::section`) so the view renders one carousel per shelf. Items
/// stay in shelf order; de-dup is *within* a shelf only (each shelf is its own
/// carousel, and the same playlist legitimately recurs across shelves).
fn parse_home(json: &Value) -> Vec<Item> {
    let mut out = Vec::new();
    let Some(sections) = json
        .pointer("/data/home/sectionContainer/sections/items")
        .and_then(Value::as_array)
    else {
        return out;
    };
    for section in sections {
        let title = section_title(section.pointer("/data/title"));
        let Some(items) = section
            .pointer("/sectionItems/items")
            .and_then(Value::as_array)
        else {
            continue;
        };
        let mut seen = HashSet::new();
        for it in items {
            if let Some(mut item) = parse_entity(it.pointer("/content/data"))
                && seen.insert(item.uri.clone())
            {
                item.section = (!title.is_empty()).then(|| title.clone());
                out.push(item);
            }
        }
    }
    out
}

/// Flatten a browse page's `data.browse.sections[]` into items tagged with their
/// shelf title. A section item is either a "Browse all" category tile
/// (`BrowseSectionContainerWrapper` → [`Kind::Category`], a page to drill into) or a
/// normal entity (playlist/album/artist). Category tiles are left un-tagged so the
/// categories root renders as one flat grid; a category's playlists keep their shelf.
fn parse_browse(json: &Value) -> Vec<Item> {
    let mut out = Vec::new();
    let Some(sections) = json
        .pointer("/data/browse/sections/items")
        .and_then(Value::as_array)
    else {
        return out;
    };
    for section in sections {
        let title = section_title(section.pointer("/data/title"));
        let Some(items) = section
            .pointer("/sectionItems/items")
            .and_then(Value::as_array)
        else {
            continue;
        };
        let mut seen = HashSet::new();
        for it in items {
            let is_category = it.pointer("/content/__typename").and_then(Value::as_str)
                == Some("BrowseSectionContainerWrapper");
            let parsed = if is_category {
                parse_category_card(it)
            } else {
                parse_entity(it.pointer("/content/data"))
            };
            if let Some(mut item) = parsed
                && seen.insert(item.uri.clone())
            {
                if item.kind != Kind::Category && !title.is_empty() {
                    item.section = Some(title.clone());
                }
                out.push(item);
            }
        }
    }
    out
}

/// A "Browse all" category tile → an openable [`Kind::Category`] item: its page uri
/// (drill target) + label + artwork from `cardRepresentation`.
fn parse_category_card(item: &Value) -> Option<Item> {
    let uri = item.get("uri")?.as_str()?.to_string();
    let card = item.pointer("/content/data/data/cardRepresentation")?;
    let name = card
        .pointer("/title/transformedLabel")
        .and_then(Value::as_str)?
        .trim()
        .to_string();
    if uri.is_empty() || name.is_empty() {
        return None;
    }
    // Canonicalise the overflow "…all categories" card (however this hub words it) so
    // the view surfaces it as a full-width button in its own row — not a tile in the
    // grid; clicking it opens the full categories page.
    let name = if is_browse_all_overflow(&name) {
        ALL_CATEGORIES_LABEL.to_string()
    } else {
        name
    };
    let image = card
        .pointer("/artwork/sources")
        .and_then(Value::as_array)
        .and_then(|s| pick_cover(s));
    // brand colour to fill the tile when it has no cover image (colour-only genres)
    let tint = card
        .pointer("/backgroundColor/hex")
        .and_then(Value::as_str)
        .and_then(parse_hex_color);
    Some(Item {
        uri,
        name,
        image,
        tint,
        kind: Kind::Category,
        ..Default::default()
    })
}

/// Parse a `#RRGGBB` hex colour into packed `0xRRGGBB`, or `None` if malformed.
fn parse_hex_color(hex: &str) -> Option<u32> {
    let h = hex.strip_prefix('#').unwrap_or(hex);
    (h.len() == 6)
        .then(|| u32::from_str_radix(h, 16).ok())
        .flatten()
}

/// A section's shelf title from its `title` node — `text` (home) or
/// `transformedLabel` (browse).
fn section_title(title: Option<&Value>) -> String {
    title
        .and_then(|t| t.get("text").or_else(|| t.get("transformedLabel")))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Map a GraphQL entity (`content.data`) to an [`Item`], or `None` if it lacks a
/// uri/name (sections, shorts, and other non-openable wrappers are skipped).
fn parse_entity(data: Option<&Value>) -> Option<Item> {
    let data = data?;
    let uri = data.get("uri")?.as_str()?.to_string();
    let name = data.get("name")?.as_str()?.trim().to_string();
    if uri.is_empty() || name.is_empty() {
        return None;
    }
    let kind = match data.get("__typename").and_then(Value::as_str).unwrap_or("") {
        "Playlist" => Kind::Playlist,
        "Album" => Kind::Album,
        "Artist" => Kind::Artist,
        "Podcast" | "Show" | "PodcastShow" => Kind::Show,
        _ => return None, // Tracks/Episodes drill in via a container, not the home grid
    };
    Some(Item {
        uri,
        name,
        subtitle: subtitle_of(data, kind),
        image: image_of(data),
        kind,
        ..Default::default()
    })
}

/// The secondary line: playlist owner or album artist(s).
fn subtitle_of(data: &Value, kind: Kind) -> String {
    match kind {
        Kind::Playlist => data
            .pointer("/ownerV2/data/name")
            .or_else(|| data.pointer("/owner/name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        Kind::Album => data
            .pointer("/artists/items")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.pointer("/profile/name").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// A crisp cover/avatar URL across the entity's image shapes (grid-card sized).
fn image_of(data: &Value) -> Option<String> {
    for ptr in [
        "/images/items/0/sources",
        "/coverArt/sources",
        "/avatarImage/sources",
        "/visuals/avatarImage/sources",
    ] {
        if let Some(url) = data
            .pointer(ptr)
            .and_then(Value::as_array)
            .and_then(|s| pick_cover(s))
        {
            return Some(url);
        }
    }
    None
}

/// Pick a cover URL big enough for a cover-art grid card: the smallest source that
/// is at least [`MIN_COVER_H`] tall (so it stays crisp when scaled to a card,
/// instead of the blurry upscale a 64px thumbnail gives), else the largest source
/// available. Sources without a declared height sort as unknown and are only used
/// as a last resort.
fn pick_cover(sources: &[Value]) -> Option<String> {
    /// Minimum cover height worth showing on a grid card. Spotify serves ~64/300/640;
    /// this lands on the ~300px tier — crisp on a card without over-fetching.
    const MIN_COVER_H: u64 = 200;
    let mut sized: Vec<(u64, String)> = sources
        .iter()
        .filter_map(|s| {
            let url = s.get("url")?.as_str()?.to_string();
            let h = s.get("height").and_then(Value::as_u64).unwrap_or(0);
            Some((h, url))
        })
        .collect();
    sized.sort_by_key(|(h, _)| *h);
    sized
        .iter()
        .find(|(h, _)| *h >= MIN_COVER_H)
        .or_else(|| sized.last())
        .map(|(_, u)| u.clone())
}

/// Percent-encode a query-string value (unreserved set only).
fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The home response nests per-entity wrappers under titled shelves, with mixed
    /// kinds, null "shorts", and within-shelf repeats. The parser must tag each item
    /// with its shelf title, de-dup within a shelf (not across), pick the right
    /// kind/subtitle/image, and skip non-openable entities.
    #[test]
    fn parse_home_tags_shelves_dedups_within_shelf_and_skips_non_entities() {
        let json: Value = serde_json::from_str(
            r#"{"data":{"home":{"sectionContainer":{"sections":{"items":[
                {"data":{"title":{"text":"Made for you"}},"sectionItems":{"items":[
                    {"content":{"data":{"__typename":"Playlist","uri":"spotify:playlist:1",
                        "name":"  Chill  ","ownerV2":{"data":{"name":"Spotify"}},
                        "images":{"items":[{"sources":[
                            {"url":"http://img/big","height":640},
                            {"url":"http://img/small","height":64}]}]}}}},
                    {"content":{"data":null}},
                    {"content":{"data":{"__typename":"Playlist","uri":"spotify:playlist:1",
                        "name":"Chill (dup in same shelf)"}}},
                    {"content":{"data":{"__typename":"Album","uri":"spotify:album:2",
                        "name":"Divide","artists":{"items":[{"profile":{"name":"Ed Sheeran"}}]},
                        "coverArt":{"sources":[{"url":"http://img/alb","height":300}]}}}}
                ]}},
                {"data":{"title":{"text":"Jump back in"}},"sectionItems":{"items":[
                    {"content":{"data":{"__typename":"Artist","uri":"spotify:artist:3",
                        "name":"Adele","avatarImage":{"sources":[{"url":"http://img/art"}]}}}},
                    {"content":{"data":{"__typename":"Track","uri":"spotify:track:9","name":"x"}}}
                ]}},
                {"data":{"title":{"text":"Podcasts for you"}},"sectionItems":{"items":[
                    {"content":{"data":{"__typename":"Podcast","uri":"spotify:show:5",
                        "name":"The Daily"}}}
                ]}}
            ]}}}}}"#,
        )
        .expect("valid fixture json");
        let items = parse_home(&json);

        // shelf 1: playlist (+ its dup removed, null skipped) + album; shelf 2: artist
        // (track dropped); shelf 3: a podcast show. 4 items, each tagged with its shelf.
        assert_eq!(items.len(), 4, "got: {items:?}");
        let pl = &items[0];
        assert_eq!(pl.kind, Kind::Playlist);
        assert_eq!(pl.name, "Chill", "edge whitespace is trimmed from names");
        assert_eq!(pl.subtitle, "Spotify", "playlist subtitle is the owner");
        assert_eq!(
            pl.section.as_deref(),
            Some("Made for you"),
            "shelf title tagged"
        );
        assert_eq!(
            pl.image.as_deref(),
            Some("http://img/big"),
            "a crisp (≥200px) cover is picked over the 64px thumbnail"
        );
        assert_eq!(items[1].kind, Kind::Album);
        assert_eq!(
            items[1].subtitle, "Ed Sheeran",
            "album subtitle is artist(s)"
        );
        assert_eq!(items[1].section.as_deref(), Some("Made for you"));
        assert_eq!(items[2].kind, Kind::Artist);
        assert_eq!(
            items[2].section.as_deref(),
            Some("Jump back in"),
            "second shelf"
        );
        assert_eq!(items[2].image.as_deref(), Some("http://img/art"));
        // a Podcast/Show entity surfaces as an openable Show card in the Home feed
        assert_eq!(
            items[3].kind,
            Kind::Show,
            "podcast shelf surfaces as a Show"
        );
        assert_eq!(items[3].name, "The Daily");
        assert_eq!(items[3].section.as_deref(), Some("Podcasts for you"));
    }

    #[test]
    fn parse_home_empty_on_shape_mismatch() {
        let json: Value = serde_json::from_str(r#"{"data":{"home":null}}"#).unwrap();
        assert!(parse_home(&json).is_empty());
    }

    /// A browse page mixes "Browse all" category tiles (BrowseSectionContainerWrapper
    /// → Kind::Category, left un-tagged so they render as one flat grid) with normal
    /// playlist shelves (tagged with their section title → carousels).
    #[test]
    fn parse_browse_reads_category_tiles_and_playlist_shelves() {
        let json: Value = serde_json::from_str(
            r##"{"data":{"browse":{"sections":{"items":[
                {"data":{"__typename":"BrowseGridSectionData","title":{"transformedLabel":"Music"}},
                 "sectionItems":{"items":[
                    {"content":{"__typename":"BrowseSectionContainerWrapper","data":{"data":{
                        "cardRepresentation":{
                            "title":{"transformedLabel":"Pop"},
                            "backgroundColor":{"hex":"#dc148c"},
                            "artwork":{"sources":[{"url":"http://img/pop","height":300,"width":300}]}}}}},
                     "uri":"spotify:page:cat-pop"},
                    {"content":{"__typename":"BrowseSectionContainerWrapper","data":{"data":{
                        "cardRepresentation":{
                            "title":{"transformedLabel":"Explore all categories"}}}}},
                     "uri":"spotify:page:all-cats"}
                 ]}},
                {"data":{"title":{"transformedLabel":"Charts"}},
                 "sectionItems":{"items":[
                    {"content":{"__typename":"PlaylistResponseWrapper","data":{
                        "__typename":"Playlist","uri":"spotify:playlist:top50","name":"Top 50",
                        "ownerV2":{"data":{"name":"Spotify"}}}}}
                 ]}}
            ]}}}}"##,
        )
        .expect("valid fixture json");
        let items = parse_browse(&json);

        assert_eq!(
            items.len(),
            3,
            "one category tile + the browse-all button + one playlist: {items:?}"
        );
        assert!(
            !items.iter().any(|i| i.name == "Explore all categories"),
            "the music hub's raw 'Explore all categories' label is canonicalised…"
        );
        assert!(
            items.iter().any(|i| i.name == ALL_CATEGORIES_LABEL),
            "…into the 'Browse all categories' button item (so it renders as the \
             shared full-width button, not a tile — same as the podcast hub)"
        );
        let cat = &items[0];
        assert_eq!(cat.kind, Kind::Category);
        assert_eq!(cat.name, "Pop");
        assert_eq!(
            cat.uri, "spotify:page:cat-pop",
            "drills into the category page"
        );
        assert_eq!(cat.image.as_deref(), Some("http://img/pop"));
        assert_eq!(
            cat.tint,
            Some(0xdc148c),
            "the card's background colour is captured for the art-less placeholder"
        );
        assert_eq!(
            cat.section, None,
            "category tiles stay flat (one grid, no shelf)"
        );
        // items[1] is the canonicalised "Browse all categories" button (also a
        // Category); the playlist from the next shelf follows it
        assert_eq!(items[1].name, ALL_CATEGORIES_LABEL);
        assert_eq!(items[2].kind, Kind::Playlist);
        assert_eq!(items[2].name, "Top 50");
        assert_eq!(
            items[2].section.as_deref(),
            Some("Charts"),
            "playlists keep their shelf"
        );
    }

    #[test]
    fn overflow_all_categories_card_is_recognised_across_hub_wordings() {
        // every hub words the "open the full categories page" card differently —
        // all fold to the one standardised button (case/whitespace-insensitive).
        for label in [
            "See all categories",
            "Browse all",
            "Explore all categories",
            "View all",
            "  SHOW ALL categories  ",
        ] {
            assert!(
                super::is_browse_all_overflow(label),
                "{label:?} should be the overflow button"
            );
        }
        // real genre/mood tiles must never be mistaken for the overflow button
        for label in ["Pop", "Hip-Hop", "Made For You", "Charts", "Allergy Relief"] {
            assert!(
                !super::is_browse_all_overflow(label),
                "{label:?} is a real category, not the overflow button"
            );
        }
    }
}
