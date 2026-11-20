//! Muted mineral colors keep session identity consistent across the feed.
use eframe::egui::{Color32, Theme};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Clone, Copy)]
pub(super) struct SessionColor {
    pub id: &'static str,
    pub name: &'static str,
    dark: Swatch,
    light: Swatch,
}

#[derive(Clone, Copy)]
pub(super) struct Swatch {
    pub accent: Color32,
    pub background: Color32,
}

impl Swatch {
    const fn new(accent: [u8; 3], background: [u8; 3]) -> Self {
        Self {
            accent: Color32::from_rgb(accent[0], accent[1], accent[2]),
            background: Color32::from_rgb(background[0], background[1], background[2]),
        }
    }

    pub fn highlighted(self) -> Color32 {
        self.background.lerp_to_gamma(self.accent, 0.14)
    }
}

pub(super) const PALETTE: [SessionColor; 6] = [
    SessionColor {
        id: "sea-glass",
        name: "Sea glass",
        dark: Swatch::new([117, 178, 177], [24, 43, 45]),
        light: Swatch::new([30, 81, 80], [220, 239, 237]),
    },
    SessionColor {
        id: "jade",
        name: "Jade",
        dark: Swatch::new([147, 182, 150], [30, 43, 33]),
        light: Swatch::new([44, 79, 51], [226, 239, 226]),
    },
    SessionColor {
        id: "slate",
        name: "Slate",
        dark: Swatch::new([143, 172, 204], [28, 38, 54]),
        light: Swatch::new([41, 70, 103], [224, 234, 246]),
    },
    SessionColor {
        id: "iris",
        name: "Iris",
        dark: Swatch::new([175, 159, 199], [40, 33, 51]),
        light: Swatch::new([78, 57, 110], [236, 229, 245]),
    },
    SessionColor {
        id: "rose-quartz",
        name: "Rose quartz",
        dark: Swatch::new([196, 156, 174], [48, 33, 43]),
        light: Swatch::new([103, 52, 73], [246, 228, 235]),
    },
    SessionColor {
        id: "brass",
        name: "Brass",
        dark: Swatch::new([195, 175, 124], [45, 41, 29]),
        light: Swatch::new([87, 69, 25], [244, 236, 214]),
    },
];

impl SessionColor {
    pub fn swatch(self, theme: Theme) -> Swatch {
        match theme {
            Theme::Dark => self.dark,
            Theme::Light => self.light,
        }
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
