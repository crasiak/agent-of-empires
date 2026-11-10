//! Applying a user action to one row or to a selection, and the unread
//! bookkeeping that follows.

use super::*;

impl HomeView {
    /// Queue a read intent after engagement. Canonical feedback clears the row.
    pub(crate) fn clear_unread_on_view(&mut self, id: &str) -> bool {
        if self.get_instance(id).is_some_and(|inst| inst.is_trashed()) {
            return false;
        }
        if self.manual_unread_hold.as_deref() == Some(id) {
            self.manual_unread_hold = None;
        }
        if self.get_instance(id).is_some_and(|i| i.is_unread()) && self.session_feed.can_submit(id)
        {
            return self
                .session_feed
                .submit(
                    id.to_owned(),
                    crate::daemon::SessionMutation::Unread(crate::daemon::UpdateUnreadBody {
                        unread: false,
                    }),
                )
                .is_ok();
        }
        false
    }

    /// Queue a read after a foreground dwell, except for a manually marked visit.
    /// Returns whether a request was admitted, not whether the row changed.
    pub fn tick_unread_dwell(&mut self, now: std::time::Instant) -> bool {
        if !crate::session::unread_enabled() || self.has_dialog() || self.selected_is_trashed() {
            self.unread_dwell = None;
            return false;
        }
        let Some(id) = self.selected_session.clone() else {
            self.unread_dwell = None;
            return false;
        };
        if self
            .manual_unread_hold
            .as_deref()
            .is_some_and(|held| held != id)
        {
            self.manual_unread_hold = None;
        }
        let started = match &self.unread_dwell {
            Some((prev, started)) if *prev == id => *started,
            _ => {
                self.unread_dwell = Some((id, now));
                return false;
            }
        };
        if now.duration_since(started) < UNREAD_DWELL {
            return false;
        }
        if self.manual_unread_hold.as_deref() == Some(id.as_str()) {
            return false;
        }
        self.clear_unread_on_view(&id)
    }

    /// Mutate a row in memory, then merge the user-owned diff under the storage
    /// flock; the daemon republishes the row from storage. Memory rolls back if
    /// persistence fails, and a row already gone from disk is left to the next
    /// canonical snapshot.
    pub(in crate::tui) fn apply_user_action<F>(&mut self, id: &str, mutate: F) -> anyhow::Result<()>
    where
        F: FnOnce(&mut Instance),
    {
        let Some(instance) = self.instances.get_mut(id) else {
            return Ok(());
        };
        let pre = instance.clone();
        mutate(instance);
        let post = instance.clone();
        self.persist_user_actions(vec![(id.to_string(), pre, post)])
    }

    /// `apply_user_action` for several rows, one storage update per profile.
    pub(in crate::tui) fn bulk_apply_user_action<F>(
        &mut self,
        ids: &[String],
        mutate: F,
    ) -> anyhow::Result<()>
    where
        F: Fn(&mut Instance),
    {
        let mut items = Vec::new();
        for id in ids {
            let Some(instance) = self.instances.get_mut(id) else {
                continue;
            };
            let pre = instance.clone();
            mutate(instance);
            items.push((id.clone(), pre, instance.clone()));
        }
        self.persist_user_actions(items)
    }

    fn persist_user_actions(
        &mut self,
        items: Vec<(String, Instance, Instance)>,
    ) -> anyhow::Result<()> {
        let mut by_profile: HashMap<String, Vec<(String, Instance, Instance)>> = HashMap::new();
        for item in items {
            by_profile
                .entry(item.2.source_profile.clone())
                .or_default()
                .push(item);
        }
        for (profile, items) in by_profile {
            let mut opened: Option<Storage> = None;
            let storage = match self.storages.get(&profile) {
                Some(storage) => Ok(storage),
                None => Storage::open_unwatched(&profile).map(|storage| &*opened.insert(storage)),
            };
            let result = storage.and_then(|storage| {
                storage.update(|rows, _groups| {
                    for (id, pre, post) in &items {
                        if let Some(disk) = rows.iter_mut().find(|row| row.id == *id) {
                            disk.merge_user_action_diff(pre, post);
                        }
                    }
                    Ok(())
                })
            });
            if let Err(error) = result {
                for (id, pre, _) in items {
                    if let Some(slot) = self.instances.get_mut(&id) {
                        *slot = pre;
                    }
                }
                return Err(error);
            }
        }
        Ok(())
    }

    /// Apply `f` to a clone and write it back even on `Err`, since resume paths
    /// can mutate conversation fields before failing.
    pub(in crate::tui) fn try_mutate_instance_writeback_on_err<T>(
        &mut self,
        id: &str,
        f: impl FnOnce(&mut Instance) -> anyhow::Result<T>,
    ) -> anyhow::Result<Option<T>> {
        let Some(instance) = self.instances.get_mut(id) else {
            return Ok(None);
        };
        let mut updated = instance.clone();
        let result = f(&mut updated);
        *instance = updated;
        result.map(Some)
    }
}
