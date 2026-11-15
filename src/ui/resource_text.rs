//! Keep the useful end of a resource name or location visible in narrow feeds.
use eframe::egui::{self, Color32, FontId};

pub(super) fn width(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(text.into(), FontId::monospace(size), Color32::WHITE)
            .size()
            .x
    })
}

pub(super) fn tail(ui: &egui::Ui, text: &str, size: f32, available: f32) -> String {
    let text = text.replace(['\n', '\r'], " ");
    let fits = |text: &str| width(ui, text, size) <= available;
    if fits(&text) {
        return text;
    }
    if !fits("…") {
        return String::new();
    }
    // Prefer cutting whole parent directories before shortening the leaf itself.
    for (index, _) in text.char_indices().filter(|(_, ch)| *ch == '/') {
        let candidate = format!("…{}", &text[index..]);
        if fits(&candidate) {
            return candidate;
        }
    }
    let boundaries: Vec<_> = text.char_indices().map(|(index, _)| index).collect();
    let mut low = 0;
    let mut high = boundaries.len();
    while low < high {
        let count = low + (high - low).div_ceil(2);
        let candidate = format!("…{}", &text[boundaries[boundaries.len() - count]..]);
        if fits(&candidate) {
            low = count;
        } else {
            high = count - 1;
        }
    }
    let suffix = if low == 0 {
        ""
    } else {
        &text[boundaries[boundaries.len() - low]..]
    };
    format!("…{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_resource_labels_preserve_the_leaf_and_unicode_suffix() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let name = "afterglow-letters-final-master.wav";
                let path = format!("/a/very/long/project/output/{name}");
                let available = width(ui, &format!("…/{name}"), 13.);
                assert_eq!(tail(ui, &path, 13., available), format!("…/{name}"));
                assert_eq!(tail(ui, name, 13., 1000.), name);
                let unicode = format!("{}.wav", "長い作品名-最終ミックス".repeat(4));
                for available in [0., 3., 40., 90., 160.] {
                    let short = tail(ui, &unicode, 13., available);
                    assert!(width(ui, &short, 13.) <= available);
                    if available >= 40. {
                        assert!(short.starts_with('…'));
                        assert!(short.ends_with(".wav"));
                    }
                }
            });
        });
    }
}
