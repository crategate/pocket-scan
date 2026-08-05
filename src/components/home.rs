use std::{
    io::Cursor,
    time::{Duration, Instant},
};

use ratatui::{prelude::*, widgets::*};
use ratatui_image::{StatefulImage, picker::Picker, protocol::StatefulProtocol};
use tokio::sync::mpsc::UnboundedSender;

use super::Component;
use crate::{action::Action, config::Config};

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
    frames: Vec<StatefulProtocol>,
    frame_durations: Vec<Duration>,
    gif_total: Duration,
    sand_count: u64,
    // dashboard state
    mesh_connected: bool,
    twitter_connected: bool,
    last_scan: Option<Instant>,
    log: Vec<String>,
    pulse: u16, // loading bar animation
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
        }
    }
}

impl Home {
    pub fn new() -> Self {
        Self::default()
    }

    fn decode_gif(&mut self) -> color_eyre::Result<()> {
        use image::{AnimationDecoder, codecs::gif::GifDecoder};
        let picker = Picker::halfblocks(); // deterministic: no terminal protocol dependency
        let decoder = GifDecoder::new(Cursor::new(GIF_BYTES))?;
        for frame in decoder.into_frames().collect_frames()? {
            let (n, d) = frame.delay().numer_denom_ms();
            let dur = Duration::from_secs_f64(n as f64 / d as f64 / 1000.0)
                .max(Duration::from_millis(30));
            self.frame_durations.push(dur);
            self.gif_total += dur;
            self.frames
                .push(picker.new_resize_protocol(frame.into_buffer().into()));
        }
        Ok(())
    }

    fn status_block<'a>(title: &'a str, connected: bool, detail: String) -> Paragraph<'a> {
        let color = if connected { Color::Green } else { Color::Red };
        let state = if connected { "ONLINE" } else { "OFFLINE" };
        Paragraph::new(vec![
            Line::from(Span::styled(state, Style::new().fg(color).bold())),
            Line::from(""),
            Line::from(detail),
        ])
        .block(
            Block::bordered()
                .title(Span::styled(format!(" {title} "), Style::new().bold()))
                .border_style(Style::new().fg(color)),
        )
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

    fn init(&mut self, _area: Size) -> color_eyre::Result<()> {
        self.decode_gif()
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
            _ => {}
        }
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> color_eyre::Result<()> {
        // Sand mode: gif covers everything
        if let Display::Sand { frame: idx, .. } = self.display {
            if !self.frames.is_empty() {
                frame.render_widget(Clear, area);
                frame.render_stateful_widget(StatefulImage::default(), area, &mut self.frames[idx]);
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
                format!("last post: {last}"),
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

        let bar_fill = ((self.pulse as f32 * 0.08).sin() * 0.5 + 0.5) as f64;
        frame.render_widget(
            Gauge::default()
                .gauge_style(Style::new().fg(Color::Yellow).bg(Color::Black))
                .ratio(bar_fill),
            bar,
        );

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

