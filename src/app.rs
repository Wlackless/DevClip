use std::{
    path::PathBuf,
    process::Command,
    sync::mpsc::TryRecvError,
    time::{Duration, Instant},
};

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use windows::Win32::Foundation::HWND;

use crate::{
    platform,
    recorder::{self, Quality, Recording},
};

const WHITE: Color32 = Color32::from_rgb(243, 243, 247);
const MUTED: Color32 = Color32::from_rgb(144, 145, 158);
const RED: Color32 = Color32::from_rgb(255, 82, 104);
const GREEN: Color32 = Color32::from_rgb(135, 229, 183);
const AMBER: Color32 = Color32::from_rgb(255, 201, 112);
const SMALL: Vec2 = Vec2::new(184.0, 44.0);
const LARGE: Vec2 = Vec2::new(410.0, 186.0);

pub struct DevClip {
    hwnd: Option<HWND>,
    quality: Quality,
    recording: Option<Recording>,
    last_hover: Instant,
    size: Vec2,
    last_tick: Instant,
    error: Option<String>,
    last_file: Option<PathBuf>,
    saved_at: Option<Instant>,
    close_requested: bool,
    privacy_ready: bool,
}

impl DevClip {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        cc.egui_ctx.style_mut(|style| {
            style.animation_time = 0.18;
            style.interaction.selectable_labels = false;
        });
        let privacy = platform::exclude_from_capture(cc);
        let (hwnd, error) = match privacy {
            Ok(hwnd) => (Some(hwnd), None),
            Err(error) => (None, Some(format!("{error:#}"))),
        };
        Self {
            hwnd,
            quality: Quality::default(),
            recording: None,
            last_hover: Instant::now() - Duration::from_secs(2),
            size: SMALL,
            last_tick: Instant::now(),
            privacy_ready: error.is_none(),
            error,
            last_file: None,
            saved_at: None,
            close_requested: false,
        }
    }

    fn start_or_pause(&mut self) {
        if let Some(recording) = &self.recording {
            if !recording.stopping && recording.has_frames() {
                recording.toggle_pause();
            }
        } else if self.privacy_ready {
            self.error = None;
            self.saved_at = None;
            self.recording = Some(Recording::start(self.quality));
        }
    }

    fn stop(&mut self) {
        if let Some(recording) = &mut self.recording {
            recording.stop();
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(recording) = &self.recording {
            let result = match recording.result.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Disconnected) => {
                    Some(Err("A gravação terminou inesperadamente".into()))
                }
                Err(TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                self.recording = None;
                match result {
                    Ok(file) => {
                        self.last_file = Some(file);
                        self.saved_at = Some(Instant::now());
                    }
                    Err(error) => self.error = Some(error),
                }
                self.last_hover = Instant::now();
                if self.close_requested {
                    if self.error.is_none() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    } else {
                        self.close_requested = false;
                    }
                }
            }
        }
    }

    fn open_folder(&mut self) {
        let directory = recorder::output_directory();
        let result = std::fs::create_dir_all(&directory).and_then(|_| {
            Command::new("explorer.exe")
                .arg(&directory)
                .spawn()
                .map(|_| ())
        });
        if let Err(error) = result {
            self.error = Some(format!("Não foi possível abrir a pasta: {error}"));
        }
    }

    fn status(&self) -> (&'static str, Color32) {
        if let Some(recording) = &self.recording {
            if recording.stopping {
                ("Salvando…", MUTED)
            } else if !recording.has_frames() {
                ("Preparando…", MUTED)
            } else if recording.is_paused() {
                ("Pausado", AMBER)
            } else {
                ("Gravando", RED)
            }
        } else if self.error.is_some() {
            ("Atenção", AMBER)
        } else if self
            .saved_at
            .is_some_and(|time| time.elapsed() < Duration::from_secs(6))
        {
            ("Vídeo salvo", GREEN)
        } else {
            ("Pronto para gravar", MUTED)
        }
    }

    fn compact(&self, ui: &mut egui::Ui, rect: Rect) {
        let (status, color) = self.status();
        let center = rect.center();
        let painter = ui.painter();
        // Tiny capture glyph and an active status dot are legible at a glance.
        let icon =
            Rect::from_center_size(Pos2::new(rect.left() + 29.0, center.y), Vec2::splat(15.0));
        painter.rect_stroke(icon, 4, Stroke::new(1.5, WHITE), StrokeKind::Inside);
        painter.circle_filled(
            icon.center(),
            3.0,
            if self.recording.is_some() { color } else { RED },
        );
        let text = if let Some(recording) = &self.recording {
            if recording.has_frames() && !recording.stopping {
                format_time(recording.elapsed())
            } else {
                status.into()
            }
        } else if self
            .saved_at
            .is_some_and(|time| time.elapsed() < Duration::from_secs(6))
        {
            "Vídeo salvo".into()
        } else {
            "DevClip".into()
        };
        painter.text(
            Pos2::new(rect.left() + 54.0, center.y),
            Align2::LEFT_CENTER,
            text,
            FontId::proportional(14.0),
            WHITE,
        );
        painter.circle_filled(Pos2::new(rect.right() - 24.0, center.y), 3.0, color);
    }

    fn expanded(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let left = rect.left() + 24.0;
        let top = rect.top();
        let (status, color) = self.status();
        ui.painter().text(
            Pos2::new(left, top + 28.0),
            Align2::LEFT_CENTER,
            "DevClip",
            FontId::proportional(19.0),
            WHITE,
        );
        ui.painter()
            .circle_filled(Pos2::new(left + 86.0, top + 28.0), 3.0, color);
        let status = if let Some(recording) = &self.recording {
            format!("{}  ·  {}", status, format_time(recording.elapsed()))
        } else {
            status.into()
        };
        ui.painter().text(
            Pos2::new(left + 98.0, top + 28.0),
            Align2::LEFT_CENTER,
            status,
            FontId::proportional(11.5),
            color,
        );

        let folder = Rect::from_min_size(
            Pos2::new(rect.right() - 80.0, top + 14.0),
            Vec2::new(27.0, 28.0),
        );
        if icon_button(ui, "folder", folder, Icon::Folder, "Abrir pasta de vídeos") {
            self.open_folder();
        }
        let close = Rect::from_min_size(
            Pos2::new(rect.right() - 49.0, top + 14.0),
            Vec2::new(27.0, 28.0),
        );
        if icon_button(
            ui,
            "close",
            close,
            Icon::Close,
            "Sair (finaliza a gravação)",
        ) {
            self.close_requested = true;
            if self.recording.is_some() {
                self.stop();
            } else {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        let busy = self.recording.is_some();
        let stopping = self.recording.as_ref().is_some_and(|r| r.stopping);
        let paused = self.recording.as_ref().is_some_and(|r| r.is_paused());
        let ready = self.recording.as_ref().is_none_or(|r| r.has_frames());
        let primary = Rect::from_min_size(
            Pos2::new(left, top + 60.0),
            Vec2::new(if busy { 240.0 } else { 362.0 }, 43.0),
        );
        let label = if stopping {
            "Salvando vídeo…"
        } else if !ready {
            "Preparando captura…"
        } else if !busy {
            "Gravar tela"
        } else if paused {
            "Continuar"
        } else {
            "Pausar"
        };
        let enabled = !stopping && ready && self.privacy_ready;
        if action_button(
            ui,
            "record",
            primary,
            label,
            if !busy {
                Icon::Record
            } else if paused {
                Icon::Play
            } else {
                Icon::Pause
            },
            if busy {
                Color32::from_rgb(40, 40, 48)
            } else {
                RED
            },
            enabled,
        ) {
            self.start_or_pause();
        }
        if busy {
            let stop =
                Rect::from_min_size(Pos2::new(left + 250.0, top + 60.0), Vec2::new(112.0, 43.0));
            if action_button(
                ui,
                "stop",
                stop,
                "Finalizar",
                Icon::Stop,
                Color32::from_rgb(40, 40, 48),
                !stopping,
            ) {
                self.stop();
            }
        }

        ui.painter().text(
            Pos2::new(left, top + 128.0),
            Align2::LEFT_CENTER,
            "QUALIDADE",
            FontId::proportional(9.0),
            MUTED,
        );
        for (i, quality) in Quality::ALL.into_iter().enumerate() {
            let chip = Rect::from_min_size(
                Pos2::new(left + 89.0 + i as f32 * 91.0, top + 115.0),
                Vec2::new(85.0, 27.0),
            );
            let selected = self.quality == quality;
            let response = ui.interact(
                chip,
                ui.id().with(("quality", i)),
                if busy { Sense::hover() } else { Sense::click() },
            );
            if selected || response.hovered() {
                ui.painter()
                    .rect_filled(chip, 8, Color32::from_rgb(38, 38, 47));
            }
            ui.painter().text(
                chip.center(),
                Align2::CENTER_CENTER,
                quality.label(),
                FontId::proportional(11.5),
                if selected { WHITE } else { MUTED },
            );
            let response = response.on_hover_text(if busy {
                "Finalize para mudar a qualidade"
            } else {
                quality.detail()
            });
            if response.clicked() {
                self.quality = quality;
            }
        }

        let footer = if self
            .saved_at
            .is_some_and(|time| time.elapsed() < Duration::from_secs(6))
            && !busy
        {
            "Salvo em Vídeos / DevClip".to_owned()
        } else {
            format!(
                "{}   ·   Tela principal   ·   Sem áudio",
                self.quality.detail()
            )
        };
        ui.painter().text(
            Pos2::new(rect.center().x, top + 162.0),
            Align2::CENTER_CENTER,
            footer,
            FontId::proportional(10.0),
            MUTED,
        );

        if let Some(error) = self.error.clone().filter(|_| rect.height() > 270.0) {
            let area = Rect::from_min_max(
                Pos2::new(left, top + 190.0),
                Pos2::new(rect.right() - 24.0, rect.bottom() - 46.0),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                egui::ScrollArea::vertical()
                    .max_height(area.height())
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(error).size(11.0).color(AMBER));
                    });
            });
            let dismiss = Rect::from_min_size(
                Pos2::new(left, rect.bottom() - 36.0),
                Vec2::new(100.0, 25.0),
            );
            if action_button(
                ui,
                "dismiss",
                dismiss,
                "Entendi",
                Icon::None,
                Color32::from_rgb(38, 38, 47),
                true,
            ) {
                self.error = None;
            }
        }
    }
}

impl eframe::App for DevClip {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.poll(ctx);
        if ctx.input(|i| i.viewport().close_requested()) && self.recording.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_requested = true;
            self.stop();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            self.start_or_pause();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.stop();
        }

        let hovered = self.hwnd.is_some_and(platform::hovered)
            || ctx.input(|i| i.pointer.hover_pos().is_some());
        if hovered {
            self.last_hover = Instant::now();
        }
        let expanded =
            self.error.is_some() || self.last_hover.elapsed() < Duration::from_millis(650);
        let target = if expanded {
            if self.error.is_some() {
                Vec2::new(LARGE.x, 330.0)
            } else {
                LARGE
            }
        } else {
            SMALL
        };
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;
        self.size += (target - self.size) * (1.0 - (-18.0 * dt).exp());
        if (self.size - target).length() < 0.7 {
            self.size = target;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(self.size));
        let scale = ctx.pixels_per_point();
        let x = (platform::screen_width() / scale - self.size.x) / 2.0;
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(Pos2::new(x, 10.0)));

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let rect = ui.max_rect().shrink(1.0);
                let radius = if self.size.y < 70.0 { 24 } else { 28 };
                ui.painter()
                    .rect_filled(rect, radius, Color32::from_rgb(16, 16, 21));
                ui.painter().rect_stroke(
                    rect,
                    radius,
                    Stroke::new(1.0, Color32::from_rgb(48, 48, 58)),
                    StrokeKind::Inside,
                );
                if self.size.x > 360.0 && self.size.y > 155.0 {
                    self.expanded(ui, rect);
                } else {
                    self.compact(ui, rect);
                }
            });
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

fn format_time(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Record,
    Pause,
    Play,
    Stop,
    Folder,
    Close,
    None,
}

fn draw_icon(painter: &egui::Painter, center: Pos2, icon: Icon, color: Color32) {
    match icon {
        Icon::Record => {
            painter.circle_filled(center, 5.0, color);
        }
        Icon::Stop => {
            painter.rect_filled(Rect::from_center_size(center, Vec2::splat(10.0)), 2, color);
        }
        Icon::Pause => {
            for x in [-3.5, 3.5] {
                painter.rect_filled(
                    Rect::from_center_size(center + Vec2::new(x, 0.0), Vec2::new(3.0, 12.0)),
                    1,
                    color,
                );
            }
        }
        Icon::Play => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    center + Vec2::new(-4.0, -6.0),
                    center + Vec2::new(6.0, 0.0),
                    center + Vec2::new(-4.0, 6.0),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Close => {
            painter.line_segment(
                [center + Vec2::splat(-4.0), center + Vec2::splat(4.0)],
                Stroke::new(1.3, color),
            );
            painter.line_segment(
                [center + Vec2::new(-4.0, 4.0), center + Vec2::new(4.0, -4.0)],
                Stroke::new(1.3, color),
            );
        }
        Icon::Folder => {
            painter.rect_stroke(
                Rect::from_center_size(center, Vec2::new(15.0, 11.0)),
                2,
                Stroke::new(1.2, color),
                StrokeKind::Inside,
            );
            painter.line_segment(
                [
                    center + Vec2::new(-6.0, -7.0),
                    center + Vec2::new(-1.0, -7.0),
                ],
                Stroke::new(1.2, color),
            );
        }
        Icon::None => {}
    }
}

fn icon_button(ui: &mut egui::Ui, id: &str, rect: Rect, icon: Icon, hint: &str) -> bool {
    let response = ui
        .interact(rect, ui.id().with(id), Sense::click())
        .on_hover_text(hint);
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 7, Color32::from_rgb(37, 37, 46));
    }
    draw_icon(ui.painter(), rect.center(), icon, MUTED);
    response.clicked()
}

fn action_button(
    ui: &mut egui::Ui,
    id: &str,
    rect: Rect,
    label: &str,
    icon: Icon,
    fill: Color32,
    enabled: bool,
) -> bool {
    let response = ui.interact(
        rect,
        ui.id().with(id),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let fill = if response.hovered() && enabled {
        fill.gamma_multiply(1.18)
    } else {
        fill
    };
    ui.painter().rect_filled(rect, 12, fill);
    let color = if enabled { WHITE } else { MUTED };
    let font = FontId::proportional(12.5);
    let width = ui
        .painter()
        .layout_no_wrap(label.into(), font.clone(), color)
        .size()
        .x;
    if matches!(icon, Icon::None) {
        ui.painter()
            .text(rect.center(), Align2::CENTER_CENTER, label, font, color);
    } else {
        let left = rect.center().x - (width + 22.0) / 2.0;
        draw_icon(
            ui.painter(),
            Pos2::new(left + 5.0, rect.center().y),
            icon,
            color,
        );
        ui.painter().text(
            Pos2::new(left + 22.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            font,
            color,
        );
    }
    response.clicked() && enabled
}
