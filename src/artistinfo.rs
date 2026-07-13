//! Online artist info (Spotify-style) — bio, genre, formed year, country.
//! Sourced from **MusicBrainz** (metadata; great international coverage) +
//! **Wikipedia** (bio extract). Fetched on a dedicated worker thread so the UI
//! never blocks on the network. No API key required.
//!
//! The bio lookup is deliberately conservative: it never shows an article that
//! isn't provably *this* artist's. A Wikipedia name can collide with a film, a
//! place, or a fictional character (e.g. "Maïssa" redirecting to a Transporter
//! film), so every name-guessed page is title-checked against the artist, and the
//! authoritative path is MusicBrainz's own Wikidata link rather than a search.

use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use serde_json::Value;

/// MusicBrainz + Wikipedia require a descriptive User-Agent (MB returns 403
/// without one).
const UA: &str = "lyrfin/0.1 ( https://github.com/lyrfin-player )";

#[derive(Debug, Clone, Default)]
pub struct ArtistInfo {
    /// `name`/`style` are parsed from the source but not yet shown in the panel.
    #[allow(dead_code)]
    pub name: String,
    pub bio: String,
    pub genre: Option<String>,
    pub formed: Option<String>,
    pub country: Option<String>,
    #[allow(dead_code)]
    pub style: Option<String>,
}

/// A fetch request (track changed → look up this artist).
#[derive(Debug, Clone)]
pub struct InfoRequest {
    pub artist: String,
    /// Reserved to refine the lookup (album-artist disambiguation); unused today.
    #[allow(dead_code)]
    pub album: String,
}

/// The worker's reply. `artist` echoes the request so the UI can ignore stale
/// results from a track the user already skipped past.
#[derive(Debug, Clone)]
pub struct InfoResult {
    pub artist: String,
    pub info: Option<ArtistInfo>,
}

/// Spawn the info worker; returns (request sender, result receiver).
pub fn spawn() -> (Sender<InfoRequest>, Receiver<InfoResult>) {
    let (req_tx, req_rx) = unbounded::<InfoRequest>();
    let (res_tx, res_rx) = unbounded::<InfoResult>();
    std::thread::Builder::new()
        .name("lyrfin-artistinfo".into())
        .spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(6)))
                .timeout_recv_body(Some(Duration::from_secs(8)))
                .build()
                .into();
            while let Ok(req) = req_rx.recv() {
                // coalesce: if more requests are queued, only serve the latest
                let mut req = req;
                while let Ok(newer) = req_rx.try_recv() {
                    req = newer;
                }
                let info = fetch_artist(&agent, &req.artist);
                let _ = res_tx.send(InfoResult {
                    artist: req.artist,
                    info,
                });
            }
        })
        .expect("spawn artistinfo thread");
    (req_tx, res_rx)
}

fn fetch_artist(agent: &ureq::Agent, name: &str) -> Option<ArtistInfo> {
    // 1. MusicBrainz: canonical name, MBID (for the Wikidata link), country, formed
    //    year, top genre tag, and the artist's native-script alias.
    let mb: Value = agent
        .get("https://musicbrainz.org/ws/2/artist")
        .header("User-Agent", UA)
        .query("query", name)
        .query("fmt", "json")
        .query("limit", "1")
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    let a = mb
        .get("artists")
        .and_then(|x| x.as_array())
        .and_then(|v| v.first());

    let (mbid, canonical, country, formed, genre, native_alias) = match a {
        Some(a) => {
            let mbid = a.get("id").and_then(Value::as_str).map(str::to_string);
            let canonical = a
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(name)
                .to_string();
            let country = a
                .get("country")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    a.get("area")
                        .and_then(|ar| ar.get("name"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
            let formed = a
                .get("life-span")
                .and_then(|l| l.get("begin"))
                .and_then(Value::as_str)
                .map(|s| s.chars().take(4).collect::<String>());
            // the artist's native-script name (alias) for their native Wikipedia
            let native_alias = country
                .as_deref()
                .and_then(country_wiki_lang)
                .and_then(|lang| alias_for_locale(a, lang));
            (mbid, canonical, country, formed, top_tag(a), native_alias)
        }
        None => (None, name.to_string(), None, None, None, None),
    };

    // 2. Bio — only ever THIS artist's article, never a name-collision. In order:
    //    (a) the English page by canonical name, accepted only if the (redirect-
    //        resolved) title matches the artist and it isn't a disambiguation page;
    //    (b) the artist's native-language page (by their native-script name);
    //    (c) the page MusicBrainz links via Wikidata — authoritative, no guessing;
    //    (d) a last-resort, title-guarded English search.
    let names: [&str; 2] = [name, &canonical];
    let mut bio = guarded_query(agent, "en", &canonical, &names).unwrap_or_default();

    let native = country.as_deref().and_then(country_wiki_lang);
    if bio.is_empty()
        && let Some(lang) = native
        && lang != "en"
    {
        if let Some(alias) = &native_alias {
            bio = guarded_query(agent, lang, alias, &[alias]).unwrap_or_default();
        }
        if bio.is_empty() {
            let alias = native_alias.as_deref().unwrap_or(&canonical);
            bio = guarded_search(agent, lang, &canonical, &[&canonical, alias]).unwrap_or_default();
        }
    }

    // Authoritative: the article MusicBrainz itself links for this exact artist.
    if bio.is_empty()
        && let Some(mbid) = &mbid
    {
        bio = mb_wikidata_bio(agent, mbid, native).unwrap_or_default();
    }

    if bio.is_empty() {
        bio = guarded_search(agent, "en", &canonical, &names).unwrap_or_default();
    }

    if bio.is_empty() && genre.is_none() && country.is_none() && formed.is_none() {
        return None;
    }
    Some(ArtistInfo {
        name: canonical,
        bio,
        genre,
        formed,
        country,
        style: None,
    })
}

/// Highest-voted MusicBrainz genre/tag.
fn top_tag(a: &Value) -> Option<String> {
    let tags = a.get("tags").and_then(Value::as_array)?;
    tags.iter()
        .max_by_key(|t| t.get("count").and_then(Value::as_i64).unwrap_or(0))
        .and_then(|t| t.get("name"))
        .and_then(Value::as_str)
        .map(title_case)
}

fn title_case(s: &str) -> String {
    s.split(' ')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A `{lang}` Wikipedia intro extract by exact title (redirects followed), but
/// only if the redirect-resolved title plausibly belongs to one of `names` — so a
/// stray redirect to an unrelated article (a film, a place, a character) is
/// rejected rather than shown as the artist's bio.
fn guarded_query(agent: &ureq::Agent, lang: &str, title: &str, names: &[&str]) -> Option<String> {
    let (resolved, extract) = wiki_query_titled(agent, lang, title)?;
    names
        .iter()
        .any(|n| name_relevant(&resolved, n))
        .then_some(extract)
}

/// Search a `{lang}` Wikipedia and return the top article's intro — but only if
/// its title plausibly belongs to one of `names` (a loose search can otherwise
/// surface a same-named film/place). Used when the exact title doesn't match.
fn guarded_search(agent: &ureq::Agent, lang: &str, query: &str, names: &[&str]) -> Option<String> {
    let (resolved, extract) = wiki_search_titled(agent, lang, query)?;
    names
        .iter()
        .any(|n| name_relevant(&resolved, n))
        .then_some(extract)
}

/// The bio from the Wikipedia article MusicBrainz links (via the artist's Wikidata
/// entity) — the authoritative, name-collision-proof source. Prefers English, then
/// the artist's native language, then any linked Wikipedia.
fn mb_wikidata_bio(agent: &ureq::Agent, mbid: &str, native: Option<&str>) -> Option<String> {
    let qid = mb_wikidata_qid(agent, mbid)?;
    let (lang, title) = wikidata_sitelink(agent, &qid, native)?;
    wiki_query_titled(agent, &lang, &title).map(|(_, extract)| extract)
}

/// The Wikidata entity id (`Q…`) MusicBrainz links for this artist, from its
/// `url-rels`. `None` if the artist has no Wikidata relation.
fn mb_wikidata_qid(agent: &ureq::Agent, mbid: &str) -> Option<String> {
    let v: Value = agent
        .get(&format!("https://musicbrainz.org/ws/2/artist/{mbid}"))
        .header("User-Agent", UA)
        .query("inc", "url-rels")
        .query("fmt", "json")
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    let rels = v.get("relations").and_then(Value::as_array)?;
    rels.iter().find_map(|r| {
        (r.get("type").and_then(Value::as_str) == Some("wikidata"))
            .then(|| {
                r.get("url")
                    .and_then(|u| u.get("resource"))
                    .and_then(Value::as_str)
            })
            .flatten()
            .and_then(wikidata_qid_from_url)
    })
}

/// Pull the `Q…` id out of a `https://www.wikidata.org/wiki/Q12345` URL.
fn wikidata_qid_from_url(url: &str) -> Option<String> {
    let id = url.trim_end_matches('/').rsplit('/').next()?;
    (id.starts_with('Q') && id[1..].chars().all(|c| c.is_ascii_digit())).then(|| id.to_string())
}

/// The `(lang, title)` of the Wikipedia article a Wikidata entity links, preferring
/// `enwiki`, then the artist's `native` language, then any real Wikipedia sitelink
/// (skipping Commons/Wikiquote/etc.).
fn wikidata_sitelink(
    agent: &ureq::Agent,
    qid: &str,
    native: Option<&str>,
) -> Option<(String, String)> {
    let v: Value = agent
        .get("https://www.wikidata.org/w/api.php")
        .header("User-Agent", UA)
        .query("action", "wbgetentities")
        .query("ids", qid)
        .query("props", "sitelinks")
        .query("format", "json")
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    let links = v.get("entities")?.get(qid)?.get("sitelinks")?.as_object()?;
    sitelink_pick(links, native)
}

/// Choose a Wikipedia sitelink: English, then `native`, then any language edition.
/// Split out (pure) so the preference order is unit-tested without the network.
fn sitelink_pick(
    links: &serde_json::Map<String, Value>,
    native: Option<&str>,
) -> Option<(String, String)> {
    let title_of = |key: &str| {
        links
            .get(key)
            .and_then(|s| s.get("title"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    if let Some(t) = title_of("enwiki") {
        return Some(("en".into(), t));
    }
    if let Some(lang) = native
        && let Some(t) = title_of(&format!("{lang}wiki"))
    {
        return Some((lang.to_string(), t));
    }
    // any language Wikipedia (e.g. "frwiki"), but not Commons/Wikiquote/Wikisource…
    links.iter().find_map(|(key, val)| {
        let lang = key.strip_suffix("wiki")?;
        if lang == "commons" || lang == "species" || lang.is_empty() {
            return None;
        }
        val.get("title")
            .and_then(Value::as_str)
            .map(|t| (lang.to_string(), t.to_string()))
    })
}

/// Plain-text intro extract from a `{lang}` Wikipedia by exact title (redirects
/// followed), returning the `(resolved_title, extract)`. Disambiguation pages are
/// skipped (they list name-collisions, not a bio).
fn wiki_query_titled(agent: &ureq::Agent, lang: &str, title: &str) -> Option<(String, String)> {
    let v: Value = agent
        .get(&format!("https://{lang}.wikipedia.org/w/api.php"))
        .header("User-Agent", UA)
        .query("action", "query")
        .query("prop", "extracts|pageprops")
        .query("ppprop", "disambiguation")
        .query("exintro", "1")
        .query("explaintext", "1")
        .query("redirects", "1")
        .query("format", "json")
        .query("titles", title)
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    extract_first_titled(&v)
}

/// Search a `{lang}` Wikipedia and return the top article's `(title, extract)`.
fn wiki_search_titled(agent: &ureq::Agent, lang: &str, query: &str) -> Option<(String, String)> {
    let v: Value = agent
        .get(&format!("https://{lang}.wikipedia.org/w/api.php"))
        .header("User-Agent", UA)
        .query("action", "query")
        .query("generator", "search")
        .query("gsrsearch", query)
        .query("gsrlimit", "1")
        .query("prop", "extracts|pageprops")
        .query("ppprop", "disambiguation")
        .query("exintro", "1")
        .query("explaintext", "1")
        .query("format", "json")
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    extract_first_titled(&v)
}

/// The first page's `(title, extract)` from a MediaWiki `query` response, skipping
/// disambiguation pages (`pageprops.disambiguation`) and empty extracts.
fn extract_first_titled(v: &Value) -> Option<(String, String)> {
    let pages = v.get("query")?.get("pages")?.as_object()?;
    for page in pages.values() {
        // a disambiguation page is a list of name-collisions, never a bio
        if page
            .get("pageprops")
            .and_then(|p| p.get("disambiguation"))
            .is_some()
        {
            continue;
        }
        let title = page
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Some(ex) = page.get("extract").and_then(Value::as_str) {
            let ex = ex.trim();
            if !ex.is_empty() {
                return Some((title, ex.to_string()));
            }
        }
    }
    None
}

/// Whether a Wikipedia page `title` plausibly belongs to `name`: the folded title
/// contains the folded artist name. Folding lowercases, maps common Latin
/// diacritics to ASCII, and drops punctuation/spaces, so "Maïssa" matches
/// "Maïssa (singer)" but not "The Transporter Refueled".
fn name_relevant(title: &str, name: &str) -> bool {
    let (t, n) = (fold(title), fold(name));
    !n.is_empty() && t.contains(&n)
}

/// Lowercase + de-accent (common Latin) + keep only alphanumerics — a loose,
/// punctuation- and diacritic-insensitive key for comparing names/titles.
fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars().flat_map(char::to_lowercase) {
        let base = deaccent(ch);
        if base.is_alphanumeric() {
            out.push(base);
        }
    }
    out
}

/// Map a (lowercased) accented Latin char to its ASCII base; pass everything else
/// (incl. non-Latin scripts) through unchanged.
fn deaccent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' => 'i',
        'ñ' | 'ń' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => 'o',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' => 'u',
        'ý' | 'ÿ' => 'y',
        'ß' => 's',
        other => other,
    }
}

/// The Wikipedia language code for an artist's `country` (an ISO code, or an area
/// name as a loose fallback) — so a non-English artist's bio comes from their own
/// language's Wikipedia, not a random one. `None` for English-speaking / unmapped
/// countries (English is already tried first, and an English search is the last
/// resort). Covers the common music markets.
fn country_wiki_lang(country: &str) -> Option<&'static str> {
    let c = country.to_uppercase();
    Some(match c.as_str() {
        "CN" | "TW" | "HK" | "MO" | "SG" => "zh",
        "JP" => "ja",
        "KR" | "KP" => "ko",
        "IL" => "he",
        "SA" | "AE" | "EG" | "MA" | "DZ" | "TN" | "IQ" | "JO" | "LB" | "KW" | "QA" | "OM"
        | "BH" | "YE" | "SY" | "LY" | "SD" | "PS" => "ar",
        "FR" => "fr",
        "DE" | "AT" => "de",
        "ES" | "MX" | "AR" | "CO" | "CL" | "PE" | "VE" | "UY" => "es",
        "PT" | "BR" => "pt",
        "IT" => "it",
        "RU" => "ru",
        "TR" => "tr",
        "NL" => "nl",
        "SE" => "sv",
        "NO" => "no",
        "FI" => "fi",
        "PL" => "pl",
        "GR" => "el",
        "TH" => "th",
        "VN" => "vi",
        "ID" => "id",
        _ => return None,
    })
}

/// A MusicBrainz alias whose locale matches `lang` (the artist's native-script
/// name) — used as an exact title in that language's Wikipedia.
fn alias_for_locale(a: &Value, lang: &str) -> Option<String> {
    a.get("aliases")
        .and_then(Value::as_array)?
        .iter()
        .find(|al| {
            al.get("locale")
                .and_then(Value::as_str)
                .is_some_and(|loc| loc.starts_with(lang))
        })
        .and_then(|al| al.get("name").and_then(Value::as_str))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn country_maps_to_the_native_wikipedia_language() {
        assert_eq!(country_wiki_lang("TW"), Some("zh"));
        assert_eq!(country_wiki_lang("cn"), Some("zh")); // case-insensitive
        assert_eq!(country_wiki_lang("IL"), Some("he"));
        assert_eq!(country_wiki_lang("EG"), Some("ar")); // Arabic only for Arab countries
        assert_eq!(country_wiki_lang("JP"), Some("ja"));
        // English-speaking / unmapped → None (English is tried first regardless)
        assert_eq!(country_wiki_lang("US"), None);
        assert_eq!(country_wiki_lang("ZZ"), None);
    }

    #[test]
    fn alias_for_locale_picks_the_native_script_name() {
        let a = serde_json::json!({
            "aliases": [
                {"name": "Dingding", "locale": "en"},
                {"name": "丁噹", "locale": "zh-Hant"},
            ]
        });
        assert_eq!(alias_for_locale(&a, "zh"), Some("丁噹".to_string()));
        assert_eq!(alias_for_locale(&a, "ja"), None);
    }

    #[test]
    fn name_relevance_accepts_the_artist_and_rejects_a_collision() {
        // the real bug: "Maïssa" must NOT accept a Transporter film article
        assert!(!name_relevant("The Transporter Refueled", "Maïssa"));
        // the artist's own page (incl. a disambiguating suffix / diacritic drift)
        assert!(name_relevant("Maïssa (singer)", "Maïssa"));
        assert!(name_relevant("Maissa", "Maïssa")); // diacritics folded both ways
        assert!(name_relevant("Boards of Canada", "Boards of Canada"));
        assert!(name_relevant("The Midnight (band)", "The Midnight"));
        // an empty name never matches (no bio rather than a false positive)
        assert!(!name_relevant("Anything", ""));
    }

    #[test]
    fn disambiguation_and_empty_pages_are_skipped() {
        // a disambiguation page is a name-collision list, not a bio → skipped
        let disambig = serde_json::json!({
            "query": { "pages": { "1": {
                "title": "Maïssa",
                "pageprops": { "disambiguation": "" },
                "extract": "Maïssa may refer to:"
            }}}
        });
        assert!(extract_first_titled(&disambig).is_none());

        let good = serde_json::json!({
            "query": { "pages": { "42": {
                "title": "Maïssa (singer)",
                "extract": "Maïssa is a French singer."
            }}}
        });
        assert_eq!(
            extract_first_titled(&good),
            Some((
                "Maïssa (singer)".into(),
                "Maïssa is a French singer.".into()
            ))
        );
    }

    #[test]
    fn wikidata_qid_is_parsed_only_from_a_valid_url() {
        assert_eq!(
            wikidata_qid_from_url("https://www.wikidata.org/wiki/Q12345"),
            Some("Q12345".into())
        );
        assert_eq!(
            wikidata_qid_from_url("https://www.wikidata.org/wiki/Q12345/"),
            Some("Q12345".into())
        );
        assert_eq!(
            wikidata_qid_from_url("https://example.com/wiki/NotAnId"),
            None
        );
        assert_eq!(
            wikidata_qid_from_url("https://www.wikidata.org/wiki/P31"),
            None
        );
    }

    #[test]
    fn sitelink_prefers_english_then_native_then_any() {
        let links = serde_json::json!({
            "enwiki": { "title": "Radiohead" },
            "frwiki": { "title": "Radiohead (FR)" },
            "commonswiki": { "title": "Category:Radiohead" }
        });
        let m = links.as_object().unwrap();
        assert_eq!(
            sitelink_pick(m, Some("fr")),
            Some(("en".into(), "Radiohead".into())),
            "English wins when present"
        );

        // no English → fall to the native language
        let native_only = serde_json::json!({
            "frwiki": { "title": "Maïssa" },
            "commonswiki": { "title": "Category:Maïssa" }
        });
        assert_eq!(
            sitelink_pick(native_only.as_object().unwrap(), Some("fr")),
            Some(("fr".into(), "Maïssa".into()))
        );

        // no English, no native → any real Wikipedia, never Commons
        let any = serde_json::json!({
            "dewiki": { "title": "Künstler" },
            "commonswiki": { "title": "Category:X" }
        });
        let picked = sitelink_pick(any.as_object().unwrap(), Some("fr"));
        assert_eq!(picked, Some(("de".into(), "Künstler".into())));
    }
}
