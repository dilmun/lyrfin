//! The keybindings overlay: the resolved `key-label → action` map loaded from
//! `keybindings.toml` over the built-in [`crate::keymap::DEFAULT_BINDINGS`]. This
//! is the *storage* side of keybindings (load/save/rebind); the `key → Action`
//! dispatch logic lives in `crate::keymap`.

use std::collections::HashMap;
use std::path::Path;

/// Resolved key → action bindings (label string → action string).
#[derive(Debug, Clone)]
pub struct Keymap {
    map: HashMap<String, String>,
}

impl Keymap {
    pub fn with_defaults() -> Self {
        let mut map = HashMap::new();
        for (k, v) in crate::keymap::DEFAULT_BINDINGS {
            map.insert((*k).to_string(), (*v).to_string());
        }
        Self { map }
    }

    /// Defaults overlaid with the user's `keybindings.toml` (sections flattened),
    /// then stale moved-default bindings migrated off (see `migrate_retired`).
    pub fn load(dir: &Path) -> Self {
        let mut km = Self::with_defaults();
        if let Ok(text) = std::fs::read_to_string(dir.join("keybindings.toml"))
            && let Ok(val) = toml::from_str::<toml::Value>(&text)
        {
            flatten_into(&val, &mut km.map);
            km.migrate_retired();
        }
        km
    }

    /// Drop any binding that still pins a key to a since-moved default (an
    /// artifact of older full-keymap dumps) and revert that key to its current
    /// built-in default, so a moved default isn't shadowed forever. See
    /// [`crate::keymap::RETIRED_BINDINGS`].
    fn migrate_retired(&mut self) {
        for (key, retired) in crate::keymap::RETIRED_BINDINGS {
            if self.map.get(*key).is_some_and(|a| a == retired) {
                match crate::keymap::DEFAULT_BINDINGS
                    .iter()
                    .find(|(k, _)| k == key)
                {
                    Some((_, def)) => self.map.insert((*key).to_string(), (*def).to_string()),
                    None => self.map.remove(*key),
                };
            }
        }
    }

    pub fn binding(&self, label: &str) -> Option<&str> {
        self.map.get(label).map(|s| s.as_str())
    }

    /// The (first, alphabetically) key label currently bound to `action`.
    pub fn label_for(&self, action: &str) -> Option<String> {
        let mut labels: Vec<&String> = self
            .map
            .iter()
            .filter(|(_, a)| a.as_str() == action && a.as_str() != "noop")
            .map(|(k, _)| k)
            .collect();
        labels.sort();
        labels.first().map(|s| (*s).clone())
    }

    /// Rebind `action` to `label`: the action's current key(s) are unbound (set
    /// to `noop` so they override any defaults) and `label` takes the action.
    pub fn rebind(&mut self, action: &str, label: &str) {
        let olds: Vec<String> = self
            .map
            .iter()
            .filter(|(_, a)| a.as_str() == action)
            .map(|(k, _)| k.clone())
            .collect();
        for o in olds {
            self.map.insert(o, "noop".into());
        }
        self.map.insert(label.to_string(), action.to_string());
    }

    /// Write only the bindings that DIFFER from the built-in defaults (the user's
    /// real overrides + additions). Unchanged defaults are omitted, so a later
    /// change to a default binding is never shadowed by a stale on-disk copy (the
    /// bug that [`Self::migrate_retired`] cleans up for older full-map dumps). No
    /// overrides → just the header.
    pub fn save(&self, dir: &Path) {
        let defaults: HashMap<&str, &str> =
            crate::keymap::DEFAULT_BINDINGS.iter().copied().collect();
        let mut entries: Vec<(&String, &String)> = self
            .map
            .iter()
            .filter(|(k, v)| defaults.get(k.as_str()).copied() != Some(v.as_str()))
            .collect();
        entries.sort();
        let mut out = String::from(
            "# lyrfin keybindings — your overrides only (\"key\" = \"action\").\n# Unlisted keys use the built-in defaults.\n",
        );
        for (k, v) in entries {
            out.push_str(&format!("{k:?} = {v:?}\n"));
        }
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join("keybindings.toml"), out);
    }
}

/// Flatten a possibly-sectioned TOML table (`[global] "q" = "quit"`) into a flat
/// key→action map.
fn flatten_into(val: &toml::Value, out: &mut HashMap<String, String>) {
    if let Some(table) = val.as_table() {
        for (k, v) in table {
            match v {
                toml::Value::String(s) => {
                    out.insert(k.clone(), s.clone());
                }
                toml::Value::Table(_) => flatten_into(v, out),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Keymap;

    #[test]
    fn retired_binding_is_migrated_off_a_stale_full_dump() {
        // simulate an upgraded user's old full-keymap dump that still pins `q` to
        // the retired default (toggle_queue moved to `u`)
        let mut km = Keymap::with_defaults();
        km.map.insert("q".into(), "toggle_queue".into());
        // also a stale speed step (0.1 → the 0.25 grid)
        km.map.insert("]".into(), "speed:+0.1".into());
        km.migrate_retired();
        // `q` reverts to its current default; the queue's new home is untouched
        assert_eq!(km.binding("q"), Some("quit"));
        assert_eq!(km.binding("u"), Some("toggle_queue"));
        assert_eq!(km.binding("]"), Some("speed:+0.25"));
    }

    #[test]
    fn migrate_leaves_a_genuine_custom_binding_alone() {
        // a user who *intentionally* bound `q` to something else keeps it — only
        // the exact retired (key, action) pair is migrated
        let mut km = Keymap::with_defaults();
        km.map.insert("q".into(), "toggle_help".into());
        km.migrate_retired();
        assert_eq!(km.binding("q"), Some("toggle_help"));
    }

    #[test]
    fn save_persists_only_overrides_and_round_trips() {
        let mut km = Keymap::with_defaults();
        km.rebind("next", "z"); // n -> noop, z -> next (two real diffs)
        let dir = std::env::temp_dir().join("lyrfin_kb_diff_test");
        let _ = std::fs::remove_dir_all(&dir);
        km.save(&dir);
        let text = std::fs::read_to_string(dir.join("keybindings.toml")).unwrap();
        // only the diffs are written — not the dozens of unchanged defaults
        assert!(text.contains("\"z\" = \"next\""));
        assert!(text.contains("\"n\" = \"noop\""));
        assert!(
            !text.contains("\"space\" = \"toggle_play\""),
            "unchanged default omitted"
        );
        assert!(
            !text.contains("\"q\" = \"quit\""),
            "unchanged default omitted"
        );
        // round-trips: loading the diff reproduces the same effective bindings,
        // and the unlisted keys still resolve to their built-in defaults
        let reloaded = Keymap::load(&dir);
        assert_eq!(reloaded.binding("z"), Some("next"));
        assert_eq!(reloaded.binding("n"), Some("noop"));
        assert_eq!(reloaded.binding("space"), Some("toggle_play"));
        assert_eq!(reloaded.binding("q"), Some("quit"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
