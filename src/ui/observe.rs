//! Observing each of the project's documents from a view.

use gpui_kit::*;

use crate::project::Project;

/// Subscriptions to each document in one of the project's lists, e.g. its PDFs. They're
/// only renewed when the list changes, not on every change to the project.
pub struct ObservedDocuments<T> {
    documents: Vec<Entity<T>>,
    _subscriptions: Vec<Subscription>,
}

impl<T: 'static> ObservedDocuments<T> {
    pub fn new() -> Self {
        Self {
            documents: Vec::new(),
            _subscriptions: Vec::new(),
        }
    }

    /// Calls `on_change` whenever one of the documents `list` picks from `project`
    /// changes. Call it again when the project changes, to catch documents being
    /// added, removed or moved.
    pub fn observe<V: 'static>(
        &mut self,
        project: &Entity<Project>,
        list: fn(&Project) -> &[Entity<T>],
        cx: &mut Context<V>,
        on_change: impl Fn(&mut V, &mut Context<V>) + Clone + 'static,
    ) {
        let documents = list(project.read(cx));
        if documents == self.documents.as_slice() {
            return;
        }
        self.documents = documents.to_vec();
        self._subscriptions = self
            .documents
            .iter()
            .map(|document| {
                let on_change = on_change.clone();
                cx.observe(document, move |view, _, cx| on_change(view, cx))
            })
            .collect();
    }
}
