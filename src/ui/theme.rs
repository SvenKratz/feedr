//! Feedr colors shared by egui widgets and custom-painted controls.
use eframe::egui::{self, Color32, Theme};

#[derive(Clone, Copy)]
pub(super) struct Palette {
    pub ink: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub paper: Color32,
    pub panel: Color32,
    pub line: Color32,
    pub hover: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub error: Color32,
}

impl Palette {
    pub fn get(ctx: &egui::Context) -> Self {
        Self::for_theme(ctx.theme())
    }

    pub fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Dark => Self {
                ink: Color32::from_rgb(217, 229, 238),
                muted: Color32::from_rgb(144, 164, 181),
                accent: Color32::from_rgb(121, 201, 203),
                paper: Color32::from_rgb(12, 17, 22),
                panel: Color32::from_rgb(16, 24, 31),
                line: Color32::from_rgb(43, 59, 73),
                hover: Color32::from_rgb(34, 50, 61),
                success: Color32::from_rgb(126, 190, 140),
                warning: Color32::from_rgb(235, 166, 142),
                error: Color32::from_rgb(235, 142, 145),
            },
            Theme::Light => Self {
                ink: Color32::from_rgb(23, 41, 50),
                muted: Color32::from_rgb(59, 80, 92),
                accent: Color32::from_rgb(17, 78, 85),
                paper: Color32::from_rgb(234, 240, 242),
                panel: Color32::from_rgb(249, 252, 253),
                line: Color32::from_rgb(168, 184, 191),
                hover: Color32::from_rgb(214, 229, 233),
                success: Color32::from_rgb(35, 86, 55),
                warning: Color32::from_rgb(130, 62, 24),
                error: Color32::from_rgb(155, 42, 54),
            },
        }
    }
}

pub(super) fn install(ctx: &egui::Context) {
    for theme in [Theme::Dark, Theme::Light] {
        let colors = Palette::for_theme(theme);
        let mut visuals = match theme {
            Theme::Dark => egui::Visuals::dark(),
            Theme::Light => egui::Visuals::light(),
        };
        visuals.panel_fill = colors.paper;
        visuals.window_fill = colors.panel;
        visuals.window_stroke.color = colors.line;
        visuals.weak_text_color = Some(colors.muted);
        visuals.hyperlink_color = colors.accent;
        visuals.warn_fg_color = colors.warning;
        visuals.error_fg_color = colors.error;
        visuals.selection.stroke = egui::Stroke::new(1.0_f32, colors.accent);
        let widgets = &mut visuals.widgets;
        if theme == Theme::Light {
            let control = Color32::from_rgb(98, 121, 132);
            let pressed = Color32::from_rgb(194, 219, 223);
            visuals.extreme_bg_color = Color32::WHITE;
            visuals.code_bg_color = Color32::from_rgb(228, 238, 241);
            visuals.selection.bg_fill = pressed;
            visuals.text_cursor.stroke.color = colors.accent;
            visuals.disabled_alpha = 0.65;

            // A visible boundary for resting controls and a stronger outline for focus.
            for (widget, fill, border) in [
                (&mut widgets.noninteractive, colors.panel, colors.line),
                (&mut widgets.inactive, colors.paper, control),
                (&mut widgets.hovered, colors.hover, colors.accent),
                (&mut widgets.active, pressed, colors.accent),
                (&mut widgets.open, colors.hover, colors.accent),
            ] {
                widget.bg_fill = fill;
                widget.weak_bg_fill = fill;
                widget.bg_stroke.color = border;
                widget.bg_stroke.width = 1.;
                widget.fg_stroke.color = colors.ink;
            }
            widgets.active.bg_stroke.width = 2.;
        } else {
            visuals.override_text_color = Some(colors.ink);
            visuals.code_bg_color = Color32::from_rgb(10, 20, 28);
            visuals.selection.bg_fill = Color32::from_rgb(34, 59, 73);
            widgets.inactive.bg_fill = Color32::from_rgb(28, 43, 54);
            widgets.inactive.weak_bg_fill = widgets.inactive.bg_fill;
            widgets.hovered.bg_fill = Color32::from_rgb(38, 61, 72);
        }
        ctx.style_mut_of(theme, |style| {
            style.visuals = visuals;
            style.spacing.item_spacing = egui::vec2(9., 8.);
            style.spacing.button_padding = egui::vec2(11., 6.);
            for (role, size) in [
                (egui::TextStyle::Body, 14.),
                (egui::TextStyle::Button, 13.),
                (egui::TextStyle::Heading, 23.),
            ] {
                style
                    .text_styles
                    .insert(role, egui::FontId::monospace(size));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::session_color;

    fn contrast(text: Color32, background: Color32) -> f32 {
        fn luminance(color: Color32) -> f32 {
            let [red, green, blue, _] = color.to_array();
            let linear = |channel| {
                let channel = f32::from(channel) / 255.;
                if channel <= 0.04045 {
                    channel / 12.92
                } else {
                    ((channel + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
        }
        let first = luminance(text);
        let second = luminance(background);
        (first.max(second) + 0.05) / (first.min(second) + 0.05)
    }

    #[test]
    fn system_appearance_changes_keep_feedr_styles_in_both_theme_slots() {
        let ctx = egui::Context::default();
        ctx.set_theme(egui::ThemePreference::System);
        // Start in light mode, including when the context had no earlier frames.
        ctx.options_mut(|options| options.fallback_theme = Theme::Light);
        install(&ctx);
        for theme in [Theme::Light, Theme::Dark, Theme::Light] {
            let _ = ctx.run(
                egui::RawInput {
                    system_theme: Some(theme),
                    ..Default::default()
                },
                |ctx| {
                    let palette = Palette::get(ctx);
                    let style = ctx.style();
                    assert_eq!(ctx.theme(), theme);
                    assert_eq!(style.visuals.dark_mode, theme == Theme::Dark);
                    assert_eq!(style.visuals.text_color(), palette.ink);
                    assert_eq!(style.visuals.panel_fill, palette.paper);
                    assert_eq!(style.visuals.window_fill, palette.panel);
                    assert_eq!(style.visuals.weak_text_color(), palette.muted);
                    assert_eq!(style.text_styles[&egui::TextStyle::Button].size, 13.);
                },
            );
        }
    }

    #[test]
    fn light_mode_keeps_labels_controls_and_session_accents_readable() {
        let ctx = egui::Context::default();
        install(&ctx);
        let style = ctx.style_of(Theme::Light);
        let visuals = &style.visuals;
        let colors = Palette::for_theme(Theme::Light);
        let text_colors = [
            colors.ink,
            colors.muted,
            colors.accent,
            colors.success,
            colors.warning,
            colors.error,
        ];
        let mut surfaces = vec![
            colors.paper,
            colors.panel,
            colors.hover,
            visuals.code_bg_color,
            visuals.text_edit_bg_color(),
            visuals.selection.bg_fill,
        ];
        for scheme in session_color::PALETTE {
            let swatch = scheme.swatch(Theme::Light);
            assert!(contrast(swatch.accent, swatch.background) >= 4.5);
            assert!(contrast(swatch.accent, swatch.highlighted()) >= 4.5);
            surfaces.extend([swatch.background, swatch.highlighted()]);
        }
        for surface in surfaces {
            for text_color in text_colors {
                assert!(
                    contrast(text_color, surface) >= 4.5,
                    "{text_color:?} on {surface:?} has insufficient contrast"
                );
            }
        }
        for widget in [
            visuals.widgets.inactive,
            visuals.widgets.hovered,
            visuals.widgets.active,
            visuals.widgets.open,
        ] {
            assert!(contrast(widget.text_color(), widget.weak_bg_fill) >= 4.5);
            assert!(contrast(widget.text_color(), widget.bg_fill) >= 4.5);
            assert!(contrast(widget.bg_stroke.color, colors.paper) >= 3.);
            assert!(contrast(widget.bg_stroke.color, colors.panel) >= 3.);
        }
        assert!(contrast(visuals.selection.stroke.color, visuals.selection.bg_fill) >= 4.5);
    }
}
