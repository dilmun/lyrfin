//! Spotify playlist WRITE operations (create / add / rename / remove / unfollow)
//! plus the account's writable-playlist fetch that backs the "add to playlist"
//! picker. Split from `client` so the read/parse worker stays focused; these are
//! the only Web API writes besides Liked Songs. Each write returns the ready-made
//! success toast (`Ok(msg)`), so the worker only has to tag it with a
//! [`PlaylistOp`] via [`write_result`].

use super::client::{get_json, playlist_item, send_json, uri_id};
use super::{API, Item, PlaylistOp, SpResult};

/// Shown when a write/fetch hits a 401 — the token lapsed mid-request (rare, since
/// it's refreshed proactively). The user just retries once reconnected.
pub(super) const AUTH_EXPIRED: &str = "Spotify session expired — reopen Spotify and try again";

/// Tag a write's `Result` with which op it was, for the app to route the refresh.
/// A 401 becomes a friendly, actionable failure toast rather than a silent retry.
pub(super) fn write_result(op: PlaylistOp, r: Result<String, Option<String>>) -> SpResult {
    match r {
        Ok(msg) => SpResult::PlaylistWrite { op, ok: true, msg },
        Err(None) => SpResult::PlaylistWrite {
            op,
            ok: false,
            msg: AUTH_EXPIRED.into(),
        },
        Err(Some(msg)) => SpResult::PlaylistWrite { op, ok: false, msg },
    }
}

/// The account's OWN (owned or collaborative) playlists — the only ones a write
/// can target. Followed-but-not-owned playlists are filtered out so the picker
/// never offers a playlist Spotify would 403. If the account id is unknown (a
/// transient profile-fetch miss), keep them all rather than showing an empty list.
pub(super) fn fetch_my_playlists(
    agent: &ureq::Agent,
    token: &str,
    user_id: &str,
) -> Result<Vec<Item>, Option<String>> {
    let v = get_json(agent, token, &format!("{API}/me/playlists?limit=50"))?;
    let items = v.get("items").and_then(|i| i.as_array());
    let Some(items) = items else {
        return Ok(Vec::new());
    };
    let writable = items
        .iter()
        .filter(|p| {
            let owned = p
                .get("owner")
                .and_then(|o| o.get("id"))
                .and_then(|x| x.as_str())
                == Some(user_id);
            let collab = p
                .get("collaborative")
                .and_then(|c| c.as_bool())
                .unwrap_or(false);
            user_id.is_empty() || owned || collab
        })
        .map(playlist_item)
        .collect();
    Ok(writable)
}

/// `POST /playlists/{id}/items` — used by both create-with-seed and a plain add.
/// Spotify's *current* add endpoint: the older `/tracks` path was deprecated and
/// now returns 403 for Development-mode apps, while `/items` works. Same
/// `{ "uris": [...] }` body (tracks + episodes), max 100 per call.
fn add_tracks_by_id(
    agent: &ureq::Agent,
    token: &str,
    id: &str,
    uris: &[String],
) -> Result<(), Option<String>> {
    let body = serde_json::json!({ "uris": uris }).to_string();
    send_json(
        agent,
        token,
        "POST",
        &format!("{API}/playlists/{id}/items"),
        &body,
    )?;
    Ok(())
}

/// Pluralize "track"/"tracks" for the toasts.
fn n_tracks(n: usize) -> String {
    format!("{n} track{}", if n == 1 { "" } else { "s" })
}

/// Create a private playlist and optionally seed it with `uris`. Uses Spotify's
/// *current* `POST /me/playlists` — the older `POST /users/{user_id}/playlists`
/// was deprecated and now 403s for Development-mode apps (and needlessly required
/// the account id).
pub(super) fn create_playlist(
    agent: &ureq::Agent,
    token: &str,
    name: &str,
    uris: &[String],
) -> Result<String, Option<String>> {
    let body = serde_json::json!({ "name": name, "public": false }).to_string();
    let v = send_json(agent, token, "POST", &format!("{API}/me/playlists"), &body)?;
    let id = v
        .get("id")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    if id.is_empty() {
        return Err(Some("Spotify didn't return the new playlist id".into()));
    }
    if uris.is_empty() {
        return Ok(format!("Created playlist “{name}”"));
    }
    add_tracks_by_id(agent, token, &id, uris)?;
    Ok(format!("Created “{name}” with {}", n_tracks(uris.len())))
}

/// Add `uris` to an existing playlist.
pub(super) fn add_tracks(
    agent: &ureq::Agent,
    token: &str,
    playlist_uri: &str,
    uris: &[String],
    name: &str,
) -> Result<String, Option<String>> {
    add_tracks_by_id(agent, token, uri_id(playlist_uri), uris)?;
    Ok(if uris.len() == 1 {
        format!("Added to “{name}”")
    } else {
        format!("Added {} to “{name}”", n_tracks(uris.len()))
    })
}

/// Rename a playlist (`PUT /playlists/{id}` with a `name`).
pub(super) fn rename(
    agent: &ureq::Agent,
    token: &str,
    playlist_uri: &str,
    name: &str,
) -> Result<String, Option<String>> {
    let body = serde_json::json!({ "name": name }).to_string();
    send_json(
        agent,
        token,
        "PUT",
        &format!("{API}/playlists/{}", uri_id(playlist_uri)),
        &body,
    )?;
    Ok(format!("Renamed to “{name}”"))
}

/// Replace a playlist's ENTIRE contents with `uris` (Spotify's "reorder or
/// replace items" endpoint, `PUT /playlists/{id}/items`). This is lyrfin's
/// remove-a-track path: personal apps can't `DELETE` playlist items (403), but
/// they CAN replace the list — so the caller sends the playlist's current tracks
/// minus the removed one. The caller MUST pass the complete current list (≤100);
/// this call trusts it and overwrites, so guarding completeness is the caller's
/// job (see `spotify_apply_remove`). `uris` may be empty (removing the last track).
pub(super) fn replace_items(
    agent: &ureq::Agent,
    token: &str,
    playlist_uri: &str,
    uris: &[String],
    name: &str,
) -> Result<String, Option<String>> {
    let body = serde_json::json!({ "uris": uris }).to_string();
    send_json(
        agent,
        token,
        "PUT",
        &format!("{API}/playlists/{}/items", uri_id(playlist_uri)),
        &body,
    )?;
    Ok(format!("Removed from “{name}”"))
}

/// Unfollow ("delete") a playlist the user owns/follows — Spotify has no delete;
/// unfollowing your own playlist is how you remove it.
pub(super) fn unfollow(
    agent: &ureq::Agent,
    token: &str,
    playlist_uri: &str,
    name: &str,
) -> Result<String, Option<String>> {
    send_json(
        agent,
        token,
        "DELETE",
        &format!("{API}/playlists/{}/followers", uri_id(playlist_uri)),
        "",
    )?;
    Ok(format!("Deleted “{name}”"))
}
