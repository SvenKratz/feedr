//! Muted mineral colors keep session identity consistent across the feed.
use super::PANEL;
use eframe::egui::Color32;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Clone, Copy)]
pub(super) struct SessionColor {
    pub id: &'static str,
    pub name: &'static str,
    pub accent: Color32,
    pub background: Color32,
}

pub(super) const PALETTE: [SessionColor; 6] = [
    SessionColor {
        id: "sea-glass",
        name: "Sea glass",
        accent: Color32::from_rgb(117, 178, 177),
        background: Color32::from_rgb(24, 43, 45),
    },
    SessionColor {
        id: "jade",
        name: "Jade",
        accent: Color32::from_rgb(147, 182, 150),
        background: Color32::from_rgb(30, 43, 33),
    },
    SessionColor {
        id: "slate",
        name: "Slate",
        accent: Color32::from_rgb(143, 172, 204),
        background: Color32::from_rgb(28, 38, 54),
    },
    SessionColor {
        id: "iris",
        name: "Iris",
        accent: Color32::from_rgb(175, 159, 199),
        background: Color32::from_rgb(40, 33, 51),
    },
    SessionColor {
        id: "rose-quartz",
        name: "Rose quartz",
        accent: Color32::from_rgb(196, 156, 174),
        background: Color32::from_rgb(48, 33, 43),
    },
    SessionColor {
        id: "brass",
        name: "Brass",
        accent: Color32::from_rgb(195, 175, 124),
        background: Color32::from_rgb(45, 41, 29),
    },
];

impl SessionColor {
    pub fn border(self) -> Color32 {
        self.background.lerp_to_gamma(self.accent, 0.3)
    }

    pub fn highlighted(self) -> Color32 {
        self.background.lerp_to_gamma(self.accent, 0.14)
    }

    pub fn sidebar(self) -> Color32 {
        self.background.lerp_to_gamma(PANEL, 0.12)
    }
}

pub(super) fn resolve(session: &str, overrides: &HashMap<String, String>) -> SessionColor {
    if let Some(id) = overrides.get(session)
        && let Some(color) = PALETTE.iter().find(|color| color.id == id)
    {
        return *color;
    }
    // Hash the durable ID, rather than a label or list position, so renaming,
    // filtering and restarting never reshuffle the session's default color.
    let hash = Sha256::digest(session.as_bytes());
    let index = u64::from_be_bytes(hash[..8].try_into().unwrap()) % PALETTE.len() as u64;
    PALETTE[index as usize]
}
