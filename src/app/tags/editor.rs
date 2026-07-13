//! Tag-editor field manipulation on `AppState` (extracted from app/tags):
//! cursor/caret movement, per-field edits, case conversion, auto-numbering,
//! filename⇄tags, find-&-replace, field removal, and the masked in-place save.

use super::*;

impl AppState {
    pub(crate) fn tag_edit_move(&mut self, m: Motion) {
        if let Some(te) = &mut self.tags.edit {
            let n = crate::tags::FIELDS.len();
            te.cursor = match m {
                Motion::Up => (te.cursor + n - 1) % n,
                Motion::Down | Motion::PageDown => (te.cursor + 1) % n,
                _ => te.cursor,
            };
            te.caret = te.draft.get(te.cursor).chars().count(); // caret to end of field
        }
    }

    /// Place the caret at the end of the focused field (entering edit mode).
    pub(crate) fn tag_edit_caret_to_end(&mut self) {
        if let Some(te) = &mut self.tags.edit {
            te.caret = te.draft.get(te.cursor).chars().count();
        }
    }

    pub(crate) fn tag_edit_caret(&mut self, c: crate::action::Caret) {
        use crate::action::Caret;
        if let Some(te) = &mut self.tags.edit {
            let len = te.draft.get(te.cursor).chars().count();
            te.caret = match c {
                Caret::Left => te.caret.saturating_sub(1),
                Caret::Right => (te.caret + 1).min(len),
                Caret::Home => 0,
                Caret::End => len,
            };
        }
    }

    /// Insert `c` at the caret (digits only for numeric fields).
    pub(crate) fn tag_edit_insert(&mut self, c: char) {
        if let Some(te) = &mut self.tags.edit {
            if crate::tags::is_numeric(te.cursor) && !c.is_ascii_digit() {
                return;
            }
            let mut chars: Vec<char> = te.draft.get(te.cursor).chars().collect();
            let caret = te.caret.min(chars.len());
            chars.insert(caret, c);
            te.caret = caret + 1;
            te.keep[te.cursor] = false;
            te.touched[te.cursor] = true;
            te.draft.set(te.cursor, chars.into_iter().collect());
        }
    }

    /// Delete the char before the caret (Backspace) or at it (`forward`).
    pub(crate) fn tag_edit_del(&mut self, forward: bool) {
        if let Some(te) = &mut self.tags.edit {
            let mut chars: Vec<char> = te.draft.get(te.cursor).chars().collect();
            let caret = te.caret.min(chars.len());
            let idx = if forward {
                if caret >= chars.len() {
                    return;
                }
                caret
            } else {
                if caret == 0 {
                    return;
                }
                caret - 1
            };
            chars.remove(idx);
            te.caret = idx;
            te.keep[te.cursor] = false;
            te.touched[te.cursor] = true;
            te.draft.set(te.cursor, chars.into_iter().collect());
        }
    }

    pub(crate) fn tag_edit_set_field(&mut self, s: String) {
        if let Some(te) = &mut self.tags.edit {
            // numeric fields accept digits only
            let v = if crate::tags::is_numeric(te.cursor) {
                s.chars().filter(|c| c.is_ascii_digit()).collect()
            } else {
                s
            };
            te.keep[te.cursor] = false; // editing commits a <keep> field
            te.touched[te.cursor] = true;
            te.draft.set(te.cursor, v);
            te.caret = te.draft.get(te.cursor).chars().count();
        }
    }

    /// Clear the focused field to empty (and mark it written — so a `<keep>`
    /// field can be emptied across the whole selection).
    pub(crate) fn tag_edit_clear(&mut self) {
        if let Some(te) = &mut self.tags.edit {
            te.keep[te.cursor] = false;
            te.touched[te.cursor] = true;
            te.draft.set(te.cursor, String::new());
        }
    }

    /// Case-transform the focused (text) field: 0 Title, 1 UPPER, 2 lower.
    pub(crate) fn tag_edit_case(&mut self, mode: u8) {
        if let Some(te) = &mut self.tags.edit {
            if crate::tags::is_numeric(te.cursor) {
                return;
            }
            let v = te.draft.get(te.cursor).to_string();
            let out = match mode {
                1 => v.to_uppercase(),
                2 => v.to_lowercase(),
                _ => title_case(&v),
            };
            te.keep[te.cursor] = false;
            te.touched[te.cursor] = true;
            te.draft.set(te.cursor, out);
        }
    }

    /// Auto-number the target tracks 1..N (and set track total), keeping any
    /// other edits the user made in the draft.
    pub(crate) fn tag_edit_autonumber(&mut self) {
        let Some(te) = self.tags.edit.take() else {
            return;
        };
        let n = te.targets.len();
        let base: Vec<bool> = (0..crate::tags::FIELDS.len())
            .map(|i| te.touched[i])
            .collect();
        let mut ok = 0usize;
        let mut last_err: Option<String> = None;
        for (i, (id, path)) in te.targets.iter().zip(te.paths.iter()).enumerate() {
            let mut e = te.draft.clone();
            e.set(4, (i + 1).to_string()); // track #
            e.set(5, n.to_string()); // track total
            let mut dirty = base.clone();
            dirty[4] = true;
            dirty[5] = true;
            match crate::tags::write_tags(path, &e, &dirty) {
                Ok(()) => {
                    if let Some(t) = self.library.track_mut(*id) {
                        e.apply_to(t, &dirty);
                    }
                    ok += 1;
                }
                Err(x) => last_err = Some(x),
            }
        }
        if base[1] || base[2] || base[3] {
            let tracks: Vec<crate::core::model::Track> =
                self.library.tracks.values().cloned().collect();
            self.library = crate::library::Library::from_tracks(tracks);
        }
        self.search.lib_gen += 1; // tags changed → refresh search index + cache
        crate::library::store::LibraryCache::from_library(&self.library).save(&self.config.dir);
        self.marks.ids.clear();
        self.notify(match last_err {
            Some(e) => format!("Numbered {ok}; error: {e}"),
            None => format!("Numbered {ok} tracks"),
        });
    }

    /// Apply the open converter prompt (filename→tags, or tags→filename rename).
    pub(crate) fn tag_convert_apply(&mut self) {
        let dir = self.tags.edit.as_ref().and_then(|t| t.convert.as_ref());
        match dir {
            Some((true, _)) => self.tag_rename_apply(),
            Some((false, _)) => self.tag_parse_filename_apply(),
            None => {}
        }
    }

    /// Tags → filename: rename each target's file from the pattern (keeps the
    /// directory + extension; skips collisions). Keeps the editor open.
    pub(crate) fn tag_rename_apply(&mut self) {
        let Some(mut te) = self.tags.edit.take() else {
            return;
        };
        let pattern = match &te.convert {
            Some((true, p)) => p.clone(),
            _ => {
                self.tags.edit = Some(te);
                return;
            }
        };
        te.convert = None;
        let mut new_paths = te.paths.clone();
        let mut ok = 0usize;
        let mut skipped = 0usize;
        let mut last_err: Option<String> = None;
        for (k, id) in te.targets.iter().enumerate() {
            let Some(track) = self.library.track(*id) else {
                continue;
            };
            let stem = crate::tags::render_pattern(&pattern, track);
            if stem.is_empty() {
                skipped += 1;
                continue;
            }
            let old = te.paths[k].clone();
            let new = match old.extension().and_then(|e| e.to_str()) {
                Some(ext) => old.with_file_name(format!("{stem}.{ext}")),
                None => old.with_file_name(&stem),
            };
            if new == old {
                ok += 1;
                continue;
            }
            if new.exists() {
                skipped += 1; // don't clobber an existing file
                continue;
            }
            match std::fs::rename(&old, &new) {
                Ok(()) => {
                    if let Some(t) = self.library.track_mut(*id) {
                        t.path = new.clone();
                    }
                    new_paths[k] = new;
                    ok += 1;
                }
                Err(e) => last_err = Some(e.to_string()),
            }
        }
        te.paths = new_paths;
        crate::library::store::LibraryCache::from_library(&self.library).save(&self.config.dir);
        self.tags.edit = Some(te); // keep the editor open
        self.notify(match last_err {
            Some(e) => format!("Renamed {ok}; error: {e}"),
            None if skipped > 0 => format!("Renamed {ok}, {skipped} skipped"),
            None => format!("Renamed {ok} files"),
        });
    }

    /// Filename → tags: parse each target's filename with the pattern and write
    /// the extracted fields per track.
    pub(crate) fn tag_parse_filename_apply(&mut self) {
        let Some(te) = self.tags.edit.take() else {
            return;
        };
        let pattern = match &te.convert {
            Some((false, p)) => p.clone(),
            _ => {
                self.tags.edit = Some(te);
                return;
            }
        };
        let tokens = crate::tags::parse_pattern(&pattern);
        let mut ok = 0usize;
        let mut skipped = 0usize;
        let mut last_err: Option<String> = None;
        let mut touched = [false; crate::tags::FIELDS.len()];
        for (id, path) in te.targets.iter().zip(te.paths.iter()) {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let Some(vals) = crate::tags::match_filename(stem, &tokens) else {
                skipped += 1;
                continue;
            };
            let mut e = crate::tags::EditableTags::default();
            let mut dirty = vec![false; crate::tags::FIELDS.len()];
            for (idx, v) in vals {
                e.set(idx, v);
                dirty[idx] = true;
                touched[idx] = true;
            }
            match crate::tags::write_tags(path, &e, &dirty) {
                Ok(()) => {
                    if let Some(t) = self.library.track_mut(*id) {
                        e.apply_to(t, &dirty);
                    }
                    ok += 1;
                }
                Err(x) => last_err = Some(x),
            }
        }
        if touched[1] || touched[2] || touched[3] {
            let tracks: Vec<crate::core::model::Track> =
                self.library.tracks.values().cloned().collect();
            self.library = crate::library::Library::from_tracks(tracks);
        }
        self.search.lib_gen += 1; // tags changed → refresh search index + cache
        crate::library::store::LibraryCache::from_library(&self.library).save(&self.config.dir);
        self.marks.ids.clear();
        self.notify(match last_err {
            Some(e) => format!("Converted {ok}; error: {e}"),
            None if skipped > 0 => format!("Converted {ok}, {skipped} skipped (no match)"),
            None => format!("Converted {ok} tracks from filename"),
        });
    }

    /// Remove the focused field's frame from every target file now (preserving
    /// cover/lyrics/unknown frames). Skips tracks where it's already empty and
    /// keeps the editor open. Distinct from `clear`, which only empties the
    /// draft and writes on save.
    pub(crate) fn tag_remove_field(&mut self) {
        let Some(mut te) = self.tags.edit.take() else {
            return;
        };
        let field = te.cursor;
        let mut dirty = vec![false; crate::tags::FIELDS.len()];
        dirty[field] = true;
        let empty = crate::tags::EditableTags::default();
        let mut ok = 0usize;
        let mut last_err: Option<String> = None;
        let mut regroup = false;
        for (id, path) in te.targets.iter().zip(te.paths.iter()) {
            // lyrics aren't in Track — read them from the file to test emptiness
            let already_empty = if field == 12 {
                crate::tags::read_lyrics(path).is_empty()
            } else {
                self.library
                    .track(*id)
                    .map(|t| {
                        crate::tags::EditableTags::from_track(t)
                            .get(field)
                            .is_empty()
                    })
                    .unwrap_or(true)
            };
            if already_empty {
                continue;
            }
            match crate::tags::write_tags(path, &empty, &dirty) {
                Ok(()) => {
                    if let Some(t) = self.library.track_mut(*id) {
                        empty.apply_to(t, &dirty);
                    }
                    regroup |= matches!(field, 1..=3);
                    ok += 1;
                }
                Err(x) => last_err = Some(x),
            }
        }
        if regroup {
            let tracks: Vec<crate::core::model::Track> =
                self.library.tracks.values().cloned().collect();
            self.library = crate::library::Library::from_tracks(tracks);
        }
        self.search.lib_gen += 1; // tags changed → refresh search index + cache
        crate::library::store::LibraryCache::from_library(&self.library).save(&self.config.dir);
        // the field is gone from the files — reflect that in the draft
        te.draft.set(field, String::new());
        te.keep[field] = false;
        te.touched[field] = false;
        let name = crate::tags::FIELDS.get(field).copied().unwrap_or("field");
        // refresh the Lyrics view if we just removed the now-playing track's lyrics
        if field == 12 && self.player.current.is_some_and(|c| te.targets.contains(&c)) {
            self.load_lyrics();
        }
        self.tags.edit = Some(te); // keep the editor open
        self.notify(match last_err {
            Some(e) => format!("Removed {name} from {ok}; error: {e}"),
            None => format!("Removed {name} from {ok} tracks"),
        });
    }

    /// Find-&-replace: substitute `find`→`repl` in the focused field across
    /// every target track (case-sensitive, all occurrences), reading each
    /// track's actual current value so bulk edits work even when the field
    /// differs across the selection. Writes only the tracks that changed and
    /// keeps the editor open.
    pub(crate) fn tag_replace_apply(&mut self) {
        let Some(mut te) = self.tags.edit.take() else {
            return;
        };
        let (find, repl) = match te.replace.take() {
            Some((f, r, _)) if !f.is_empty() => (f, r),
            _ => {
                self.tags.edit = Some(te); // nothing to find — just close the prompt
                return;
            }
        };
        let field = te.cursor;
        let mut ok = 0usize;
        let mut last_err: Option<String> = None;
        let mut regroup = false;
        for (id, path) in te.targets.iter().zip(te.paths.iter()) {
            let Some(track) = self.library.track(*id) else {
                continue;
            };
            let cur = crate::tags::EditableTags::from_track(track);
            let val = cur.get(field);
            if !val.contains(&find) {
                continue;
            }
            let mut e = cur.clone();
            e.set(field, val.replace(&find, &repl));
            let mut dirty = vec![false; crate::tags::FIELDS.len()];
            dirty[field] = true;
            match crate::tags::write_tags(path, &e, &dirty) {
                Ok(()) => {
                    if let Some(t) = self.library.track_mut(*id) {
                        e.apply_to(t, &dirty);
                    }
                    regroup |= matches!(field, 1..=3);
                    ok += 1;
                }
                Err(x) => last_err = Some(x),
            }
        }
        if regroup {
            let tracks: Vec<crate::core::model::Track> =
                self.library.tracks.values().cloned().collect();
            self.library = crate::library::Library::from_tracks(tracks);
        }
        self.search.lib_gen += 1; // tags changed → refresh search index + cache
        crate::library::store::LibraryCache::from_library(&self.library).save(&self.config.dir);
        // reflect the change in the visible draft (focused field, first target)
        if let Some(t) = te.targets.first().and_then(|id| self.library.track(*id)) {
            te.draft.set(
                field,
                crate::tags::EditableTags::from_track(t)
                    .get(field)
                    .to_string(),
            );
            te.keep[field] = false;
        }
        self.tags.edit = Some(te); // keep the editor open
        self.notify(match last_err {
            Some(e) => format!("Replaced in {ok}; error: {e}"),
            None => format!("Replaced in {ok} tracks"),
        });
    }

    /// Write the edited fields to every target file and refresh the library.
    /// Only fields the user actually changed (and didn't leave as `<keep>`) are
    /// written; everything else in each file is preserved.
    pub(crate) fn tag_edit_save(&mut self) {
        self.tag_edit_apply(false);
    }

    /// Write the manual-edit draft (only the fields the user touched). `album`
    /// expands the write set from the editor's own targets to every track in the
    /// first target's album (apply-to-album).
    pub(crate) fn tag_edit_apply(&mut self, album: bool) {
        let Some(mut te) = self.tags.edit.take() else {
            return;
        };
        te.confirm_album = false; // consumed — don't re-arm if we re-open on error
        let n = crate::tags::FIELDS.len();
        // a field is written iff the user touched it (typed/cleared/transformed) —
        // so emptying a <keep> field clears it across the whole selection
        let dirty: Vec<bool> = (0..n).map(|i| te.touched[i]).collect();
        if !dirty.iter().any(|&d| d) {
            self.notify("No changes".into());
            self.tags.edit = Some(te); // keep the editor open
            return;
        }

        // resolve the write set: the editor's own selection, or the whole album
        // of the first target when applying album-wide.
        let (targets, paths): (Vec<crate::core::model::TrackId>, Vec<std::path::PathBuf>) = if album
        {
            let aid = te
                .targets
                .first()
                .and_then(|id| self.library.track(*id))
                .and_then(|t| t.album_id);
            match aid {
                Some(a) => {
                    let ids: Vec<_> = self.library.tracks_of(a).iter().map(|t| t.id).collect();
                    let paths = ids
                        .iter()
                        .filter_map(|id| self.library.track(*id))
                        .map(|t| t.path.clone())
                        .collect();
                    (ids, paths)
                }
                None => (te.targets.clone(), te.paths.clone()),
            }
        } else {
            (te.targets.clone(), te.paths.clone())
        };

        let mut ok = 0usize;
        let mut last_err: Option<String> = None;
        for (id, path) in targets.iter().zip(paths.iter()) {
            match crate::tags::write_tags(path, &te.draft, &dirty) {
                Ok(()) => {
                    if let Some(t) = self.library.track_mut(*id) {
                        te.draft.apply_to(t, &dirty);
                    }
                    ok += 1;
                }
                Err(e) => last_err = Some(e),
            }
        }

        // every write failed: keep the editor open (don't lose the edits) and
        // surface the real error so we can see what's blocking the file.
        if ok == 0 {
            let e = last_err.unwrap_or_else(|| "unknown".into());
            // longer-lived so the (possibly long) lofty error can be read
            self.notification = Some(Notification {
                text: format!("Write failed: {e}"),
                ttl_ticks: 600,
            });
            self.tags.edit = Some(te);
            return;
        }

        // artist/album tree grouping only depends on these fields
        if dirty[1] || dirty[2] || dirty[3] {
            let tracks: Vec<crate::core::model::Track> =
                self.library.tracks.values().cloned().collect();
            self.library = crate::library::Library::from_tracks(tracks);
        }
        self.search.lib_gen += 1; // tags changed → refresh search index + cache
        crate::library::store::LibraryCache::from_library(&self.library).save(&self.config.dir);
        // lyrics edited for the now-playing track → refresh the Lyrics view
        if dirty.get(12).copied().unwrap_or(false)
            && self.player.current.is_some_and(|c| targets.contains(&c))
        {
            self.load_lyrics();
        }
        self.marks.ids.clear();
        self.close_tags(); // success closes the whole unified modal
        self.notify(match (last_err, ok) {
            (Some(e), _) => format!("Saved {ok}, some failed: {e}"),
            (None, 1) => "Tags saved".into(),
            (None, k) => format!("Tags saved to {k} tracks"),
        });
    }
}
