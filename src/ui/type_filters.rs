//! Independent, persisted resource-type toggles; no selection means All.
use crate::model::FeedQuery;

pub(super) const TYPES: [&str; 8] = [
    "Markdown", "Image", "PDF", "Video", "Audio", "Web", "Folder", "File",
];
const STORAGE_KEY: &str = "feedr.resource_types";

pub(super) fn label(kind: &str) -> &str {
    if kind == "Web" { "HTML" } else { kind }
}

pub(super) fn restore(storage: Option<&dyn eframe::Storage>) -> Vec<String> {
    let saved: Vec<String> = storage
        .and_then(|storage| eframe::get_value(storage, STORAGE_KEY))
        .unwrap_or_default();
    TYPES
        .into_iter()
        .filter(|kind| saved.iter().any(|value| value == kind))
        .map(str::to_owned)
        .collect()
}

pub(super) fn save(storage: &mut dyn eframe::Storage, query: &FeedQuery) {
    eframe::set_value(
        storage,
        STORAGE_KEY,
        &query.selected_types().collect::<Vec<_>>(),
    );
}

pub(super) fn selected(query: &FeedQuery, kind: &str) -> bool {
    if kind == "All" {
        query.selected_types().next().is_none()
    } else {
        query.selected_types().any(|selected| selected == kind)
    }
}

pub(super) fn toggle(query: &mut FeedQuery, kind: &str) {
    let was_selected = selected(query, kind);
    let mut kinds: Vec<String> = TYPES
        .into_iter()
        .filter(|value| selected(query, value))
        .map(str::to_owned)
        .collect();
    if kind == "All" {
        kinds.clear();
    } else if was_selected {
        kinds.retain(|value| value != kind);
    } else if TYPES.contains(&kind) {
        kinds.push(kind.into());
    }
    query.kind = None;
    query.kinds = kinds;
}
