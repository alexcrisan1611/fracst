//! The Ctrl+K palette: type a few letters of anything the app does, and run it.

use crate::theme;
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Id, Key, Modifiers, Order, Rect, RichText, Sense, Stroke, TextEdit,
    Vec2,
};

/// Whether an entry is a setting that is on, off, or neither.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Action,
    On,
    Off,
}

/// Something the palette can run.
#[derive(Clone, Debug)]
pub struct Entry<T> {
    pub action: T,
    pub title: String,
    /// Where it lives, shown on the right: "File", "Colour", …
    pub group: &'static str,
    pub shortcut: Option<String>,
    pub state: State,
    /// Stable across runs, to remember recently used entries.
    pub key: String,
}

#[derive(Default)]
pub struct Search {
    pub open: bool,
    query: String,
    selected: usize,
    /// Keys taken this frame, before anything else could act on them.
    up: bool,
    down: bool,
    enter: bool,
    /// Keep the selection in view after moving it with the keyboard.
    scroll: bool,
}

const WIDTH: f32 = 580.0;

impl Search {
    pub fn toggle(&mut self) {
        if self.open {
            self.open = false;
        } else {
            self.open_with("");
        }
    }

    pub fn open_with(&mut self, query: &str) {
        self.open = true;
        self.query = query.to_owned();
        self.selected = 0;
    }

    /// Take Ctrl+K, and while open the keys the palette uses, before the
    /// rest of the app sees them (Esc would otherwise cancel the render).
    pub fn take_keys(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::K)) {
            self.toggle();
        }
        if !self.open {
            return;
        }
        ctx.input_mut(|i| {
            self.up = i.consume_key(Modifiers::NONE, Key::ArrowUp);
            self.down = i.consume_key(Modifiers::NONE, Key::ArrowDown);
            self.enter = i.consume_key(Modifiers::NONE, Key::Enter);
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                self.open = false;
            }
        });
    }

    /// Show the palette; returns the entry chosen.
    pub fn show<T: Clone>(&mut self, ctx: &egui::Context, entries: &[Entry<T>], recent: &[String]) -> Option<Entry<T>> {
        if !self.open {
            return None;
        }
        let screen = ctx.content_rect();
        // The dimmed backdrop takes clicks outside the palette, so they
        // close it without zooming the canvas underneath.
        let backdrop = egui::Area::new(Id::unique("search-backdrop"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(screen.size(), Sense::CLICK | Sense::DRAG);
                ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha(90));
                response
            })
            .inner;
        if backdrop.clicked() || backdrop.secondary_clicked() {
            self.open = false;
            return None;
        }

        let results = rank(&self.query, entries, recent);
        if self.down {
            self.selected = (self.selected + 1).min(results.len().saturating_sub(1));
            self.scroll = true;
        }
        if self.up {
            self.selected = self.selected.saturating_sub(1);
            self.scroll = true;
        }
        self.selected = self.selected.min(results.len().saturating_sub(1));
        let mut chosen = (self.enter && !results.is_empty()).then_some(self.selected);
        (self.up, self.down, self.enter) = (false, false, false);

        let top = screen.top() + (screen.height() * 0.12).max(60.0);
        // Above the dimming, which is above everything else.
        egui::Area::new(Id::unique("search"))
            .order(Order::Tooltip)
            .pivot(Align2::CENTER_TOP)
            .fixed_pos(egui::pos2(screen.center().x, top))
            .show(ctx, |ui| {
                theme::card().fill(theme::SOLID_FILL).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                    ui.set_width(WIDTH.min(screen.width() - 40.0));
                    let edit = ui.add(
                        TextEdit::singleline(&mut self.query)
                            .hint_text("Search actions, settings and places…")
                            .font(egui::FontId::proportional(16.0))
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::symmetric(8, 8))
                            .desired_width(f32::INFINITY),
                    );
                    edit.request_focus();
                    if edit.changed() {
                        self.selected = 0;
                    }
                    ui.separator();
                    if results.is_empty() {
                        ui.add_space(6.0);
                        ui.label(RichText::new("Nothing matches.").color(theme::TEXT_WEAK));
                        ui.add_space(6.0);
                        return;
                    }
                    let max_height = (screen.height() * 0.55).min(420.0);
                    egui::ScrollArea::vertical().max_height(max_height).auto_shrink([false, true]).show(ui, |ui| {
                        let mut last_group = None;
                        for (row, &(i, recent)) in results.iter().enumerate() {
                            let group = if recent { "Recent" } else { entries[i].group };
                            if self.query.is_empty() && last_group != Some(group) {
                                ui.add_space(if last_group.is_some() { 6.0 } else { 2.0 });
                                ui.label(RichText::new(group).small().color(theme::TEXT_WEAK));
                                last_group = Some(group);
                            }
                            let response = entry_row(ui, &entries[i], row == self.selected, !self.query.is_empty());
                            if response.clicked() {
                                chosen = Some(row);
                            }
                            if response.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                                self.selected = row;
                            }
                            if row == self.selected && self.scroll {
                                response.scroll_to_me(Some(Align::Center));
                                self.scroll = false;
                            }
                        }
                    });
                });
            });
        let entry = chosen.and_then(|row| results.get(row)).map(|&(i, _)| entries[i].clone());
        if entry.is_some() {
            self.open = false;
        }
        entry
    }
}

fn entry_row<T>(ui: &mut egui::Ui, entry: &Entry<T>, selected: bool, show_group: bool) -> egui::Response {
    let height = 30.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    let painter = ui.painter();
    let accent = ui.visuals().hyperlink_color;
    if selected {
        painter.rect_filled(rect, CornerRadius::same(7), accent.gamma_multiply(0.22));
        painter.rect_stroke(
            rect,
            CornerRadius::same(7),
            Stroke::new(1.0, accent.gamma_multiply(0.5)),
            egui::StrokeKind::Inside,
        );
    } else if response.hovered() {
        painter.rect_filled(rect, CornerRadius::same(7), Color32::from_white_alpha(8));
    }
    let mut x = rect.left() + 10.0;
    let y = rect.center().y;
    // A switch for on/off settings.
    if entry.state != State::Action {
        let on = entry.state == State::On;
        let track = Rect::from_center_size(egui::pos2(x + 11.0, y), Vec2::new(22.0, 12.0));
        let fill = if on { accent } else { Color32::from_gray(70) };
        painter.rect_filled(track, CornerRadius::same(6), fill);
        let knob = if on { track.right() - 6.0 } else { track.left() + 6.0 };
        painter.circle_filled(egui::pos2(knob, y), 4.5, Color32::WHITE);
        x += 32.0;
    }
    let title = painter.layout_no_wrap(entry.title.clone(), egui::FontId::proportional(14.0), theme::TEXT);
    let mut right = rect.right() - 10.0;
    if let Some(shortcut) = &entry.shortcut {
        let galley = painter.layout_no_wrap(shortcut.clone(), egui::FontId::monospace(11.5), theme::TEXT_WEAK);
        let cap = Rect::from_min_size(
            egui::pos2(right - galley.size().x - 12.0, y - galley.size().y / 2.0 - 3.0),
            galley.size() + Vec2::new(12.0, 6.0),
        );
        painter.rect_stroke(
            cap,
            CornerRadius::same(5),
            Stroke::new(1.0, Color32::from_gray(70)),
            egui::StrokeKind::Inside,
        );
        painter.galley(cap.min + Vec2::new(6.0, 3.0), galley, theme::TEXT_WEAK);
        right = cap.left() - 8.0;
    }
    if show_group {
        let galley = painter.layout_no_wrap(entry.group.to_owned(), egui::FontId::proportional(12.0), theme::TEXT_WEAK);
        let pos = egui::pos2(right - galley.size().x, y - galley.size().y / 2.0);
        right = pos.x - 8.0;
        painter.galley(pos, galley, theme::TEXT_WEAK);
    }
    let clip = Rect::from_min_max(rect.min, egui::pos2(right, rect.max.y));
    painter.with_clip_rect(clip).galley(egui::pos2(x, y - title.size().y / 2.0), title, theme::TEXT);
    response
}

/// The entries to list, best first, with whether each is listed as recent.
fn rank<T>(query: &str, entries: &[Entry<T>], recent: &[String]) -> Vec<(usize, bool)> {
    let recency = |key: &str| recent.iter().position(|k| k == key);
    if query.trim().is_empty() {
        // Recently used first, then everything in its usual order.
        let mut out: Vec<(usize, bool)> =
            recent.iter().filter_map(|k| entries.iter().position(|e| &e.key == k)).take(5).map(|i| (i, true)).collect();
        let listed: Vec<usize> = out.iter().map(|&(i, _)| i).collect();
        out.extend((0..entries.len()).filter(|i| !listed.contains(i)).map(|i| (i, false)));
        return out;
    }
    let mut scored: Vec<(i64, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let s = score(query, &format!("{} {}", e.title, e.group))?;
            // A small nudge for what was used lately.
            let bonus = recency(&e.key).map_or(0, |r| 12 - r.min(10) as i64);
            Some((s + bonus, i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| (i, false)).collect()
}

/// How well `query` matches `text`: every word of the query must appear in
/// order, letters may be skipped. Higher is better; `None` is no match.
pub fn score(query: &str, text: &str) -> Option<i64> {
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut total = 0;
    for word in query.to_lowercase().split_whitespace() {
        total += word_score(word, &text)?;
    }
    Some(total)
}

fn word_score(word: &str, text: &[char]) -> Option<i64> {
    let word: Vec<char> = word.chars().collect();
    // Try every place the first letter starts a match, keep the best.
    let mut best = None;
    for start in (0..text.len()).filter(|&i| text[i] == word[0]) {
        let mut score = 0i64;
        let mut at = start;
        let mut prev: Option<usize> = None;
        let mut ok = true;
        for &c in &word {
            let Some(found) = (at..text.len()).find(|&j| text[j] == c) else {
                ok = false;
                break;
            };
            let word_start = found == 0 || !text[found - 1].is_alphanumeric();
            score += 1;
            if word_start {
                score += 8;
            }
            match prev {
                Some(p) if found == p + 1 => score += 6,
                Some(p) => score -= (found - p - 1).min(6) as i64,
                None if found == 0 => score += 10,
                None => {}
            }
            prev = Some(found);
            at = found + 1;
        }
        if ok {
            best = best.max(Some(score));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, group: &'static str, key: &str) -> Entry<()> {
        Entry { action: (), title: title.into(), group, shortcut: None, state: State::Action, key: key.into() }
    }

    #[test]
    fn matches_words_and_initials() {
        assert!(score("slope", "Slope shading Colour").is_some());
        assert!(score("sl sh", "Slope shading Colour").is_some());
        assert!(score("exp png", "Export PNG… File").is_some());
        assert!(score("xyz", "Slope shading").is_none());
        // Starting words and runs of letters beat scattered letters.
        assert!(score("zo", "Zoom out") > score("zo", "Iteration zone"));
        assert!(score("zoom", "Zoom out") > score("zoom", "Z-order of mom"));
    }

    #[test]
    fn ranks_best_first_and_lists_recent_when_empty() {
        let entries = [
            entry("Export PNG…", "File", "export"),
            entry("Open location…", "File", "open"),
            entry("Slope shading", "Colour", "slopes"),
        ];
        let ranked = rank("open", &entries, &[]);
        assert_eq!(ranked[0].0, 1);
        let ranked = rank("", &entries, &["slopes".into()]);
        assert_eq!(ranked, vec![(2, true), (0, false), (1, false)]);
        // Unknown recent keys are skipped.
        assert_eq!(rank("", &entries, &["gone".into()]).len(), 3);
    }
}
