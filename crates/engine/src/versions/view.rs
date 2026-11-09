//! A previous version opened from Version History, as the session keeps it.
//!
//! DeckCraft keeps several presentations open, so a version is shown beside the presentation it
//! came from, as a read only presentation of its own: it has no file, nothing in it can be changed
//! (`cmd::version_view` refuses the commands that would), and closing it puts the presentation it
//! came from in front again, exactly as it was, because it was never touched.

use super::VersionInfo;
use crate::Session;

/// Why an edit is refused while a previous version is shown (the status line shows it).
pub const READ_ONLY: &str = "This is a previous version. Restore it or save a copy to make changes.";

/// The marker a presentation carries while it is a previous version.
#[derive(Clone, Debug)]
pub struct PreviousVersion {
    /// The uid of the presentation whose history the version belongs to.
    pub source: u64,
    /// The version's record (when it was saved, by whom).
    pub info: VersionInfo,
}

impl Session {
    /// The previous version shown by presentation `index`, if it is one.
    pub fn previous_version(&self, index: usize) -> Option<&PreviousVersion> {
        self.docs.get(index)?.previous.as_deref()
    }

    /// The previous version in front now, if the active presentation is one.
    pub fn active_previous_version(&self) -> Option<&PreviousVersion> {
        self.previous_version(self.active?)
    }

    /// The index of the previous version of presentation `source` (its uid) that is open.
    pub fn previous_version_of(&self, source: u64) -> Option<usize> {
        self.docs.iter().position(|d| d.previous.as_ref().is_some_and(|p| p.source == source))
    }

    /// The presentation whose history presentation `index` has: itself, or for a previous version
    /// the presentation it came from. `None` when that one has been closed.
    pub fn history_owner(&self, index: usize) -> Option<usize> {
        match self.docs.get(index)?.previous.as_deref() {
            Some(previous) => self.index_of_uid(previous.source),
            None => Some(index),
        }
    }

    /// Close previous version `index` and put the presentation it came from in front. Returns the
    /// index of that presentation (`None` when presentation `index` is not a previous version).
    pub fn close_previous_version(&mut self, index: usize) -> Option<usize> {
        let source = self.docs.get(index)?.previous.as_ref()?.source;
        self.close_document(index);
        let at = self.index_of_uid(source);
        if let Some(i) = at {
            self.set_active(i);
        }
        at
    }

    /// Close every previous version of presentation `source` (its uid) without changing which
    /// presentation is in front, unless it was one of them.
    pub(crate) fn drop_previous_versions(&mut self, source: u64) {
        while let Some(i) = self.previous_version_of(source) {
            self.close_previous_version(i);
        }
    }

    /// Close every previous version and put the presentation in front that was in front before
    /// it: what New, Open and quitting do first, so a shown version never hides the presentation.
    pub fn close_previous_versions(&mut self) {
        let active = self.active;
        let uid = active.and_then(|i| self.docs.get(i)).map(|d| d.previous.as_ref().map_or(d.uid, |p| p.source));
        while let Some(i) = self.docs.iter().position(|d| d.previous.is_some()) {
            self.close_previous_version(i);
        }
        if let Some(i) = uid.and_then(|u| self.index_of_uid(u)) {
            self.set_active(i);
        }
    }
}
