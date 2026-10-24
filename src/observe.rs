//! Observing each document in a list, e.g. the project's PDFs.

use gpui_kit::*;

/// Subscriptions to each document in a list. They're only renewed when the list
/// changes, not every time it's passed again.
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

    /// Calls `on_change` whenever one of `documents` changes. Call it again when the
    /// list may have changed, to catch documents being added, removed or moved.
    pub fn observe<V: 'static>(
        &mut self,
        documents: &[Entity<T>],
        cx: &mut Context<V>,
        on_change: impl Fn(&mut V, &mut Context<V>) + Clone + 'static,
    ) {
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

    /// Stops observing the documents.
    pub fn clear(&mut self) {
        self.documents.clear();
        self._subscriptions.clear();
    }
}
