//! The queue drawer (D-Queue): 360 px over the right of the content, the
//! one region with a drop shadow, with the autoplay toggle and an
//! "Autoplay radio" section for radio entries.
//!
//! Integrator stub (the old queue panel). The `player` worker owns this
//! file. `app.rs` places the drawer; [`show`] fills it.

use egui::{CornerRadius, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Ui, vec2};

use crate::state::{Command, State};
use crate::theme;
use crate::views::{Out, View, artists, cmd};
use crate::widgets::{ArtKind, art, icon_button, section, text_line};

/// Drag payload for reordering the queue: the dragged entry's index.
struct QueueDrag(usize);

pub fn show(ui: &mut Ui, state: &State, _view: &mut View, out: &mut Out) {
    ui.add_space(8.0);
    ui.label(
        RichText::new("Queue")
            .size(18.0)
            .strong()
            .color(theme::pal().text_1),
    );
    ui.add_space(6.0);
    let q = &state.queue;
    section(ui, "NOW PLAYING");
    match q.current_index() {
        Some(i) => queue_row(ui, state, i, true, out),
        None => {
            ui.label(RichText::new("Nothing playing").color(theme::pal().text_3));
        }
    }
    ui.add_space(8.0);
    let start = q.upcoming_start();
    let upcoming = if q.is_empty() {
        0
    } else {
        q.len().saturating_sub(start)
    };
    section(ui, &format!("NEXT UP · {upcoming}"));
    if upcoming == 0 {
        ui.label(
            RichText::new("Right-click a song and pick \"Add to queue\".")
                .color(theme::pal().text_3),
        );
        return;
    }
    ScrollArea::vertical()
        .id_salt("queue")
        .auto_shrink([false, false])
        .show_rows(ui, QUEUE_ROW, upcoming, |ui, range| {
            for k in range {
                queue_row(ui, state, start + k, false, out);
            }
        });
}

const QUEUE_ROW: f32 = 50.0;

fn queue_row(ui: &mut Ui, state: &State, index: usize, is_current: bool, out: &mut Out) {
    let q = &state.queue;
    let entry = &q.entries()[index];
    let sense = if is_current {
        Sense::click()
    } else {
        Sense::click_and_drag()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), QUEUE_ROW), sense);
    let painter = ui.painter_at(rect);
    if resp.hovered() || resp.dragged() {
        painter.rect_filled(rect, CornerRadius::same(6), theme::pal().surface_2);
    }
    let art_rect = Rect::from_min_size(
        Pos2::new(rect.left() + 6.0, rect.center().y - 19.0),
        vec2(38.0, 38.0),
    );
    art(
        &painter,
        art_rect,
        &entry.track.id,
        &entry.track.title,
        ArtKind::Square(&entry.track.thumbnails),
    );
    let x = art_rect.right() + 10.0;
    let w = rect.right() - x - 34.0;
    let color = if is_current {
        theme::accent()
    } else {
        theme::pal().text_1
    };
    text_line(
        &painter,
        Pos2::new(x, rect.center().y - 8.0),
        &entry.track.title,
        theme::row(),
        color,
        w,
    );
    text_line(
        &painter,
        Pos2::new(x, rect.center().y + 10.0),
        &artists(&entry.track),
        theme::secondary(),
        theme::pal().text_2,
        w,
    );

    if !is_current {
        resp.dnd_set_drag_payload(QueueDrag(index));
        if resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        let pointer_y = ui.ctx().pointer_interact_pos().map_or(0.0, |p| p.y);
        let above = pointer_y < rect.center().y;
        if resp.dnd_hover_payload::<QueueDrag>().is_some() {
            let y = if above { rect.top() } else { rect.bottom() };
            ui.painter()
                .hline(rect.x_range(), y, Stroke::new(2.0, theme::accent()));
        }
        if let Some(drag) = resp.dnd_release_payload::<QueueDrag>() {
            let slot = if above { index } else { index + 1 };
            let from = drag.0;
            let to = if from < slot { slot - 1 } else { slot };
            if from != to {
                cmd(out, Command::MoveEntry { from, to });
            }
        }
        let remove_rect = Rect::from_center_size(
            Pos2::new(rect.right() - 16.0, rect.center().y),
            vec2(24.0, 24.0),
        );
        if (resp.hovered() || ui.rect_contains_pointer(remove_rect))
            && icon_button(
                ui,
                remove_rect,
                &format!("rm{index}"),
                crate::icons::X,
                13.0,
                false,
                "Remove",
            )
            .clicked()
        {
            cmd(out, Command::RemoveEntry(index));
        }
    }
    if resp.double_clicked() {
        cmd(out, Command::JumpTo(index));
    }
    let start = q.upcoming_start();
    let last = q.len().saturating_sub(1);
    resp.context_menu(|ui| {
        if ui.button("Play now").clicked() {
            cmd(out, Command::JumpTo(index));
            ui.close();
        }
        if !is_current {
            if index > start && ui.button("Move up").clicked() {
                cmd(
                    out,
                    Command::MoveEntry {
                        from: index,
                        to: index - 1,
                    },
                );
                ui.close();
            }
            if index < last && ui.button("Move down").clicked() {
                cmd(
                    out,
                    Command::MoveEntry {
                        from: index,
                        to: index + 1,
                    },
                );
                ui.close();
            }
            if index > start && ui.button("Move to top").clicked() {
                cmd(
                    out,
                    Command::MoveEntry {
                        from: index,
                        to: start,
                    },
                );
                ui.close();
            }
        }
        if ui.button("Remove from queue").clicked() {
            cmd(out, Command::RemoveEntry(index));
            ui.close();
        }
    });
}
