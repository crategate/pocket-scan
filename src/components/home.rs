use std::{
    io::Cursor,
    time::{Duration, Instant},
};

use super::Component;
use crate::{action::Action, config::Config};
use ratatui::{prelude::*, widgets::*};
use ratatui_image::{Image, Resize, picker::Picker, protocol::Protocol};
use tokio::sync::mpsc::UnboundedSender;
use tracing::info;

static GIF_BYTES: &[u8] = include_bytes!("../../assets/pocket-sand.gif");

enum Display {
    Dashboard,
    Sand {
        started: Instant,
        frame: usize,
        last_advance: Instant,
    },
}

pub struct Home {
    command_tx: Option<UnboundedSender<Action>>,
    config: Config,
    display: Display,
    frames: Vec<Protocol>,
    frame_durations: Vec<Duration>,
    gif_total: Duration,
    sand_count: u64,
    // dashboard state
    mesh_connected: bool,
    twitter_connected: bool,
    last_scan: Option<Instant>,
    log: Vec<String>,
    pulse: u16, // loading bar animation
    x_posts: u64,
    x_cents: u64,
}

impl Default for Home {
    fn default() -> Self {
        Self {
            command_tx: None,
            config: Config::default(),
            display: Display::Dashboard,
            frames: Vec::new(),
            frame_durations: Vec::new(),
            gif_total: Duration::ZERO,
            sand_count: 0,
            mesh_connected: false,
            twitter_connected: false,
            last_scan: None,
            log: Vec::new(),
            pulse: 0,
            x_posts: 0,
            x_cents: 0,
        }
    }
}

impl Home {
    pub fn new() -> Self {
        Self::default()
    }
    fn decode_gif(&mut self, area: Size) -> color_eyre::Result<()> {
        use image::{AnimationDecoder, codecs::gif::GifDecoder};
        let picker = Picker::halfblocks();
        let fs = picker.font_size(); // (10, 20) for halfblocks
        let target_w = area.width as u32 * fs.width as u32;
        let target_h = area.height as u32 * fs.height as u32;
        let decoder = GifDecoder::new(Cursor::new(GIF_BYTES))?;
        info!("decode_gif start: {}x{} cells", area.width, area.height);
        for frame in decoder.into_frames().collect_frames()? {
            let (n, d) = frame.delay().numer_denom_ms();
            let dur = Duration::from_secs_f64(n as f64 / d as f64 / 1000.0)
                .max(Duration::from_millis(30));
            self.frame_durations.push(dur);
            self.gif_total += dur;
            let img: image::DynamicImage = frame.into_buffer().into();
            let img = img.resize_exact(target_w, target_h, image::imageops::FilterType::Triangle);
            self.frames
                .push(picker.new_protocol(img, area, Resize::Fit(None))?);
        }
        info!(
            "decode_gif done: {} frames, total {:?}",
            self.frames.len(),
            self.gif_total
        );
        Ok(())
    }

    fn status_block<'a>(title: &'a str, connected: bool, detail: String) -> Paragraph<'a> {
        let border_color = if connected { Color::Green } else { Color::Red };
        let state = if connected { "ONLINE" } else { "OFFLINE" };

        // Base style: light red alert fill when down, default black when up
        let base = if connected {
            Style::default()
        } else {
            Style::new().bg(Color::LightRed).fg(Color::Black)
        };
        let state_style = if connected {
            Style::new().fg(Color::Green).bold()
        } else {
            Style::new().fg(Color::Black).bold()
        };

        Paragraph::new(vec![
            Line::from(Span::styled(state, state_style)),
            Line::from(""),
            Line::from(Span::styled(detail, base)),
        ])
        .block(
            Block::bordered()
                .title(Span::styled(format!(" {title} "), Style::new().bold()))
                .border_style(Style::new().fg(border_color)),
        )
        .style(base)
        .alignment(Alignment::Center)
    }
}

impl Component for Home {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> color_eyre::Result<()> {
        self.command_tx = Some(tx);
        Ok(())
    }

    fn register_config_handler(&mut self, config: Config) -> color_eyre::Result<()> {
        self.config = config;
        Ok(())
    }

    fn init(&mut self, area: Size) -> color_eyre::Result<()> {
        // self.decode_gif()
        self.decode_gif(area)
    }

    fn update(&mut self, action: Action) -> color_eyre::Result<Option<Action>> {
        match action {
            Action::SandStart => {
                self.sand_count += 1;
                self.display = Display::Sand {
                    started: Instant::now(),
                    frame: 0,
                    last_advance: Instant::now(),
                };
            }
            Action::Scan(payload) => {
                self.last_scan = Some(Instant::now());
                self.log.push(format!("SCAN  {payload}"));
                // TODO later: forward to mesh + twitter workers via command_tx
            }
            Action::Tick => {
                self.pulse = self.pulse.wrapping_add(1);
                // GIF finished? back to the dashboard
                if let Display::Sand { started, .. } = &self.display {
                    if started.elapsed() >= self.gif_total {
                        self.display = Display::Dashboard;
                    }
                }
            }
            Action::Render => {
                if let Display::Sand {
                    frame,
                    last_advance,
                    ..
                } = &mut self.display
                {
                    if last_advance.elapsed() >= self.frame_durations[*frame] {
                        *frame = (*frame + 1) % self.frames.len();
                        *last_advance = Instant::now();
                    }
                }
            }
            Action::MeshStatus(connected) => {
                self.mesh_connected = connected;
                self.log.push(format!(
                    "MESH  {}",
                    if connected {
                        "connected"
                    } else {
                        "disconnected — retrying"
                    }
                ));
            }
            Action::MeshTx { ok, text } => {
                let short: String = text.chars().take(48).collect();
                self.log.push(format!(
                    "MESH  {} {short}",
                    if ok { "→ sent" } else { "✗ FAILED" }
                ));
            }
            Action::MeshRx { from, text } => {
                let short: String = text.chars().take(48).collect();
                self.log.push(format!("MESH  ← {from}: {short}"));
            }
            Action::XStatus(connected) => {
                self.twitter_connected = connected;
                self.log.push(format!(
                    "X     {}",
                    if connected {
                        "creds loaded"
                    } else {
                        "no creds — idle"
                    }
                ));
            }
            Action::XTx { ok, text } => {
                if ok {
                    self.x_posts += 1;
                    self.x_cents += if text.contains("http") { 20 } else { 2 }; // $0.20 / $0.015
                }
                let short: String = text.chars().take(48).collect();
                self.log.push(format!(
                    "X     {} {short}",
                    if ok { "→ posted" } else { "✗" }
                ));
            }
            _ => {}
        }
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> color_eyre::Result<()> {
        // Sand mode: gif covers everything
        if let Display::Sand { frame: idx, .. } = self.display {
            if !self.frames.is_empty() {
                frame.render_widget(Clear, area);
                frame.render_widget(Image::new(&self.frames[idx]), area);
            }
            return Ok(());
        }

        // Dashboard: two top tiles / loading bar / log pane
        let [top, bar, bottom] = Layout::vertical([
            Constraint::Percentage(55),
            Constraint::Length(1),
            Constraint::Min(4),
        ])
        .areas(area);
        let [twitter_area, mesh_area] =
            Layout::horizontal([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)]).areas(top);

        let last = self
            .last_scan
            .map(|t| format!("{}s ago", t.elapsed().as_secs()))
            .unwrap_or_else(|| "never".into());

        frame.render_widget(
            Self::status_block(
                "X / TWITTER",
                self.twitter_connected,
                format!(
                    "posts: {} | damage: ${:.2}",
                    self.x_posts,
                    self.x_cents as f64 / 100.0
                ),
            ),
            twitter_area,
        );
        frame.render_widget(
            Self::status_block(
                "MESHTASTIC",
                self.mesh_connected,
                format!("sand thrown: {}", self.sand_count),
            ),
            mesh_area,
        );

        //        let bar_fill = ((self.pulse as f32 * 0.08).sin() * 0.5 + 0.5) as f64;
        //        frame.render_widget(
        //            Gauge::default()
        //                .gauge_style(Style::new().fg(Color::Yellow).bg(Color::Black))
        //                .ratio(bar_fill),
        //            bar,
        //        );

        let items: Vec<ListItem> = self
            .log
            .iter()
            .rev()
            .take(bottom.height.saturating_sub(2) as usize)
            .map(|e| ListItem::new(e.clone()))
            .collect();
        frame.render_widget(
            List::new(items).block(Block::bordered().title(" SCANS / POSTS ")),
            bottom,
        );
        Ok(())
    }
}
