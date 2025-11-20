use color_eyre::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::{FutureExt, StreamExt};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let terminal = ratatui::init();
    let result = App::new().run(terminal).await;
    ratatui::restore();
    result
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ModelInfo {
    #[serde(rename = "modelId")]
    id: String,
    author: Option<String>,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    likes: u64,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(rename = "lastModified", default)]
    last_modified: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Normal,
    Editing,
}

#[derive(Debug)]
pub struct App {
    running: bool,
    event_stream: EventStream,
    input: Input,
    input_mode: InputMode,
    models: Arc<Mutex<Vec<ModelInfo>>>,
    list_state: ListState,
    loading: bool,
    error: Option<String>,
    status: String,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        
        Self {
            running: false,
            event_stream: EventStream::default(),
            input: Input::default(),
            input_mode: InputMode::Normal,
            models: Arc::new(Mutex::new(Vec::new())),
            list_state,
            loading: false,
            error: None,
            status: "Press '/' to search, 'q' to quit".to_string(),
        }
    }

    pub async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        self.running = true;
        while self.running {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_crossterm_events().await?;
        }
        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(10),
                Constraint::Length(3),
            ])
            .split(frame.area());

        // Search input box
        let input_block = Block::default()
            .borders(Borders::ALL)
            .title("Search HuggingFace Models")
            .border_style(if self.input_mode == InputMode::Editing {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            });

        let width = input_block.inner(chunks[0]).width.max(3) - 1;
        let scroll = self.input.visual_scroll(width as usize);
        
        let input_widget = Paragraph::new(self.input.value())
            .style(Style::default())
            .block(input_block)
            .scroll((0, scroll as u16));
        
        frame.render_widget(input_widget, chunks[0]);

        if self.input_mode == InputMode::Editing {
            frame.set_cursor_position((
                chunks[0].x + ((self.input.visual_cursor()).max(scroll) - scroll) as u16 + 1,
                chunks[0].y + 1,
            ));
        }

        // Results list
        let models = futures::executor::block_on(async {
            self.models.lock().await.clone()
        });

        let items: Vec<ListItem> = models
            .iter()
            .enumerate()
            .map(|(idx, model)| {
                let author = model.author.as_deref().unwrap_or("unknown");
                let downloads = format_number(model.downloads);
                let likes = format_number(model.likes);
                
                let tags_str = if model.tags.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", model.tags.iter().take(3).cloned().collect::<Vec<_>>().join(", "))
                };

                let content = Line::from(vec![
                    Span::styled(
                        format!("{:3}. ", idx + 1),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        &model.id,
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" by "),
                    Span::styled(author, Style::default().fg(Color::Green)),
                    Span::raw(format!(" ↓{} ♥{}", downloads, likes)),
                    Span::styled(tags_str, Style::default().fg(Color::Yellow)),
                ]);

                ListItem::new(content)
            })
            .collect();

        let list_title = if self.loading {
            "Results [Loading...]"
        } else if models.is_empty() && !self.input.value().is_empty() {
            "Results [No models found]"
        } else if models.is_empty() {
            "Results [Enter a search query]"
        } else {
            "Results"
        };

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(list_title)
                    .border_style(if self.input_mode == InputMode::Normal {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default()
                    }),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(">> ");

        frame.render_stateful_widget(list, chunks[1], &mut self.list_state);

        // Status bar
        let status_text = if let Some(err) = &self.error {
            format!("Error: {}", err)
        } else if !models.is_empty() {
            if let Some(selected) = self.list_state.selected() {
                if selected < models.len() {
                    let model = &models[selected];
                    format!(
                        "Selected: {} | URL: https://huggingface.co/{}",
                        model.id, model.id
                    )
                } else {
                    self.status.clone()
                }
            } else {
                self.status.clone()
            }
        } else {
            self.status.clone()
        };

        let status = Paragraph::new(status_text)
            .block(Block::default().borders(Borders::ALL).title("Status"))
            .style(if self.error.is_some() {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::White)
            })
            .wrap(Wrap { trim: true });

        frame.render_widget(status, chunks[2]);
    }

    async fn handle_crossterm_events(&mut self) -> Result<()> {
        let event = self.event_stream.next().fuse().await;
        match event {
            Some(Ok(evt)) => {
                if let Event::Key(key) = evt {
                    if key.kind == KeyEventKind::Press {
                        self.on_key_event(key).await;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn on_key_event(&mut self, key: KeyEvent) {
        self.error = None;

        match self.input_mode {
            InputMode::Normal => match (key.modifiers, key.code) {
                (_, KeyCode::Char('q'))
                | (KeyModifiers::CONTROL, KeyCode::Char('c') | KeyCode::Char('C')) => self.quit(),
                (_, KeyCode::Char('/')) => {
                    self.input_mode = InputMode::Editing;
                    self.status = "Enter search query, press Enter to search, ESC to cancel".to_string();
                }
                (_, KeyCode::Down | KeyCode::Char('j')) => self.next(),
                (_, KeyCode::Up | KeyCode::Char('k')) => self.previous(),
                (_, KeyCode::Enter) => {
                    self.show_model_details().await;
                }
                _ => {}
            },
            InputMode::Editing => match key.code {
                KeyCode::Enter => {
                    self.input_mode = InputMode::Normal;
                    self.status = "Searching...".to_string();
                    self.search_models().await;
                }
                KeyCode::Esc => {
                    self.input_mode = InputMode::Normal;
                    self.status = "Press '/' to search, 'q' to quit".to_string();
                }
                _ => {
                    self.input.handle_event(&Event::Key(key));
                }
            },
        }
    }

    fn next(&mut self) {
        let models_len = futures::executor::block_on(async {
            self.models.lock().await.len()
        });
        
        if models_len == 0 {
            return;
        }
        
        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= models_len - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn previous(&mut self) {
        let models_len = futures::executor::block_on(async {
            self.models.lock().await.len()
        });
        
        if models_len == 0 {
            return;
        }
        
        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    models_len - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    async fn search_models(&mut self) {
        let query = self.input.value().to_string();
        
        if query.is_empty() {
            return;
        }

        self.loading = true;
        self.error = None;
        
        let models = self.models.clone();
        
        match fetch_models(&query).await {
            Ok(results) => {
                let mut models_lock = models.lock().await;
                *models_lock = results;
                self.loading = false;
                self.list_state.select(Some(0));
                self.status = format!("Found {} models", models_lock.len());
            }
            Err(e) => {
                self.loading = false;
                self.error = Some(format!("Failed to fetch models: {}", e));
                self.status = "Search failed".to_string();
            }
        }
    }

    async fn show_model_details(&mut self) {
        let models = self.models.lock().await;
        if let Some(selected) = self.list_state.selected() {
            if selected < models.len() {
                let model = &models[selected];
                self.status = format!(
                    "{} | Downloads: {} | Likes: {} | Tags: {}",
                    model.id,
                    format_number(model.downloads),
                    format_number(model.likes),
                    if model.tags.is_empty() {
                        "none".to_string()
                    } else {
                        model.tags.join(", ")
                    }
                );
            }
        }
    }

    fn quit(&mut self) {
        self.running = false;
    }
}

async fn fetch_models(query: &str) -> Result<Vec<ModelInfo>, reqwest::Error> {
    let url = format!(
        "https://huggingface.co/api/models?search={}&limit=50&sort=downloads&direction=-1",
        urlencoding::encode(query)
    );
    
    let response = reqwest::get(&url).await?;
    let models: Vec<ModelInfo> = response.json().await?;
    
    Ok(models)
}

fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
