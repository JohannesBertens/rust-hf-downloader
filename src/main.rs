use color_eyre::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::{FutureExt, StreamExt};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap, Gauge, Clear},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;
use std::fs;
use std::io::Write;

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

#[derive(Debug, Clone, Deserialize)]
struct ModelFile {
    #[serde(rename = "type")]
    file_type: String,
    path: String,
    #[serde(default)]
    size: u64,
}

#[derive(Debug, Clone)]
struct QuantizationInfo {
    quant_type: String,
    filename: String,
    size: u64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct DownloadProgress {
    model_id: String,
    filename: String,
    downloaded: u64,
    total: u64,
    speed_mbps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
enum DownloadStatus {
    Incomplete,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DownloadMetadata {
    model_id: String,
    filename: String,
    url: String,
    local_path: String,
    total_size: u64,
    downloaded_size: u64,
    status: DownloadStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct DownloadRegistry {
    downloads: Vec<DownloadMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PopupMode {
    None,
    DownloadPath,
    ResumeDownload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Normal,
    Editing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusedPane {
    Models,
    Quantizations,
}

#[derive(Debug)]
pub struct App {
    running: bool,
    event_stream: EventStream,
    input: Input,
    input_mode: InputMode,
    focused_pane: FocusedPane,
    models: Arc<Mutex<Vec<ModelInfo>>>,
    list_state: ListState,
    quant_list_state: ListState,
    loading: bool,
    error: Option<String>,
    status: String,
    quantizations: Arc<Mutex<Vec<QuantizationInfo>>>,
    loading_quants: bool,
    quant_cache: Arc<Mutex<HashMap<String, Vec<QuantizationInfo>>>>,
    popup_mode: PopupMode,
    download_path_input: Input,
    download_progress: Arc<Mutex<Option<DownloadProgress>>>,
    download_tx: mpsc::UnboundedSender<(String, String, PathBuf)>,
    download_rx: Arc<Mutex<mpsc::UnboundedReceiver<(String, String, PathBuf)>>>,
    download_queue_size: Arc<Mutex<usize>>,
    incomplete_downloads: Vec<DownloadMetadata>,
    status_rx: Arc<Mutex<mpsc::UnboundedReceiver<String>>>,
    status_tx: mpsc::UnboundedSender<String>,
    download_registry: Arc<Mutex<DownloadRegistry>>,
    complete_downloads: Arc<Mutex<HashMap<String, DownloadMetadata>>>, // Key: filename
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
        
        let quant_list_state = ListState::default();
        
        let (download_tx, download_rx) = mpsc::unbounded_channel();
        let (status_tx, status_rx) = mpsc::unbounded_channel();
        
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let default_path = format!("{}/models", home);
        let mut download_path_input = Input::default();
        download_path_input = download_path_input.with_value(default_path);
        
        Self {
            running: false,
            event_stream: EventStream::default(),
            input: Input::default(),
            input_mode: InputMode::Editing,  // Start in editing mode for immediate search
            focused_pane: FocusedPane::Models,
            models: Arc::new(Mutex::new(Vec::new())),
            list_state,
            quant_list_state,
            loading: false,
            error: None,
            status: "Enter search query, press Enter to search, ESC to browse, 'q' to quit".to_string(),
            quantizations: Arc::new(Mutex::new(Vec::new())),
            loading_quants: false,
            quant_cache: Arc::new(Mutex::new(HashMap::new())),
            popup_mode: PopupMode::None,
            download_path_input,
            download_progress: Arc::new(Mutex::new(None)),
            download_tx,
            download_rx: Arc::new(Mutex::new(download_rx)),
            download_queue_size: Arc::new(Mutex::new(0)),
            incomplete_downloads: Vec::new(),
            status_rx: Arc::new(Mutex::new(status_rx)),
            status_tx,
            download_registry: Arc::new(Mutex::new(DownloadRegistry::default())),
            complete_downloads: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    
    fn get_registry_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(format!("{}/models/hf-downloads.toml", home))
    }
    
    fn load_registry() -> DownloadRegistry {
        let path = Self::get_registry_path();
        if !path.exists() {
            return DownloadRegistry::default();
        }
        
        match fs::read_to_string(&path) {
            Ok(content) => {
                toml::from_str(&content).unwrap_or_default()
            }
            Err(_) => DownloadRegistry::default(),
        }
    }
    
    fn save_registry(registry: &DownloadRegistry) {
        let path = Self::get_registry_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        
        if let Ok(toml_string) = toml::to_string_pretty(registry) {
            if let Ok(mut file) = fs::File::create(&path) {
                let _ = file.write_all(toml_string.as_bytes());
            }
        }
    }

    async fn scan_incomplete_downloads(&mut self) {
        // Load registry from disk
        let registry = Self::load_registry();
        
        // Update the app's registry
        {
            let mut reg = self.download_registry.lock().await;
            *reg = registry.clone();
        }
        
        // Find incomplete downloads
        self.incomplete_downloads = registry.downloads.iter()
            .filter(|d| d.status == DownloadStatus::Incomplete)
            .cloned()
            .collect();
        
        // Load complete downloads into memory (keyed by filename for quick lookup)
        let complete_map: HashMap<String, DownloadMetadata> = registry.downloads.into_iter()
            .filter(|d| d.status == DownloadStatus::Complete)
            .map(|d| (d.filename.clone(), d))
            .collect();
        
        {
            let mut complete = self.complete_downloads.lock().await;
            *complete = complete_map;
        }
        
        // Show popup if incomplete downloads found
        if !self.incomplete_downloads.is_empty() {
            self.popup_mode = PopupMode::ResumeDownload;
            self.status = format!("Found {} incomplete download(s)", self.incomplete_downloads.len());
        }
    }

    pub async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        self.running = true;
        
        // Scan for incomplete downloads on startup
        self.scan_incomplete_downloads().await;
        
        // Spawn download manager task
        let download_rx = self.download_rx.clone();
        let download_progress = self.download_progress.clone();
        let download_queue_size = self.download_queue_size.clone();
        let status_tx = self.status_tx.clone();
        let complete_downloads = self.complete_downloads.clone();
        tokio::spawn(async move {
            let mut rx = download_rx.lock().await;
            while let Some((model_id, filename, path)) = rx.recv().await {
                // Decrement queue size when we start processing
                {
                    let mut queue_size = download_queue_size.lock().await;
                    *queue_size = queue_size.saturating_sub(1);
                }
                start_download(model_id, filename, path, download_progress.clone(), status_tx.clone(), complete_downloads.clone()).await;
            }
        });
        
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
                Constraint::Length(12),
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
                // Extract author from model.id if not provided (e.g., "unsloth/model" -> "unsloth")
                let author = model.author.as_deref()
                    .or_else(|| model.id.split('/').next())
                    .unwrap_or("unknown");
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
                    .border_style(
                        if self.input_mode == InputMode::Normal 
                            && self.focused_pane == FocusedPane::Models {
                            Style::default().fg(Color::Yellow)
                        } else {
                            Style::default()
                        }
                    ),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(">> ");

        frame.render_stateful_widget(list, chunks[1], &mut self.list_state);

        // Quantization details panel
        let quantizations = futures::executor::block_on(async {
            self.quantizations.lock().await.clone()
        });
        
        let complete_downloads = futures::executor::block_on(async {
            self.complete_downloads.lock().await.clone()
        });

        let quant_title = if self.loading_quants {
            "Quantization Details [Loading...]"
        } else if quantizations.is_empty() {
            "Quantization Details [Select a model to view]"
        } else {
            "Quantization Details"
        };

        let quant_items: Vec<ListItem> = quantizations
            .iter()
            .map(|q| {
                let size_str = format_size(q.size);
                let is_downloaded = complete_downloads.contains_key(&q.filename);
                
                let mut spans = vec![
                    Span::raw(format!("{:>10}  ", size_str)),
                    Span::styled(
                        format!("{:<14} ", q.quant_type),
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    ),
                ];
                
                if is_downloaded {
                    spans.push(Span::styled(&q.filename, Style::default().fg(Color::Green)));
                    spans.push(Span::styled(" [downloaded]", Style::default().fg(Color::Green)));
                } else {
                    spans.push(Span::styled(&q.filename, Style::default().fg(Color::White)));
                }
                
                let content = Line::from(spans);
                ListItem::new(content)
            })
            .collect();

        let quant_list = List::new(quant_items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(quant_title)
                    .border_style(
                        if self.input_mode == InputMode::Normal 
                            && self.focused_pane == FocusedPane::Quantizations {
                            Style::default().fg(Color::Yellow)
                        } else {
                            Style::default()
                        }
                    ),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(">> ");

        frame.render_stateful_widget(quant_list, chunks[2], &mut self.quant_list_state);

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

        frame.render_widget(status, chunks[3]);
        
        // Render download progress bar in top right corner if download is active
        let (download_progress, queue_size) = futures::executor::block_on(async {
            let progress = self.download_progress.lock().await.clone();
            let queue = *self.download_queue_size.lock().await;
            (progress, queue)
        });
        
        if let Some(progress) = download_progress {
            let progress_area = Rect {
                x: frame.area().width.saturating_sub(42),
                y: 0,
                width: 42.min(frame.area().width),
                height: 3,
            };
            
            let percentage = if progress.total > 0 {
                (progress.downloaded as f64 / progress.total as f64 * 100.0) as u16
            } else {
                0
            };
            
            // Format title with queue info
            let title = if queue_size > 0 {
                format!("Downloading ({} queued)", queue_size)
            } else {
                "Downloading".to_string()
            };
            
            // Format label with percentage and speed
            let label = if progress.speed_mbps > 0.0 {
                format!("{}% - {:.2} MB/s", percentage, progress.speed_mbps)
            } else {
                format!("{}%", percentage)
            };
            
            let gauge = Gauge::default()
                .block(Block::default().borders(Borders::ALL).title(title))
                .gauge_style(Style::default().fg(Color::Cyan).bg(Color::Black))
                .percent(percentage)
                .label(label);
            
            frame.render_widget(gauge, progress_area);
        }
        
        // IMPORTANT: Render popup last so it appears on top of all other widgets
        if self.popup_mode == PopupMode::ResumeDownload {
            // Calculate centered popup area
            let popup_width = 70.min(frame.area().width.saturating_sub(4));
            let popup_height = 10 + self.incomplete_downloads.len().min(5) as u16;
            let popup_x = (frame.area().width.saturating_sub(popup_width)) / 2;
            let popup_y = (frame.area().height.saturating_sub(popup_height)) / 2;
            
            let popup_area = Rect {
                x: popup_x,
                y: popup_y,
                width: popup_width,
                height: popup_height,
            };
            
            // Clear the popup area first to remove any underlying content
            frame.render_widget(Clear, popup_area);
            
            // Render popup background
            let popup_block = Block::default()
                .borders(Borders::ALL)
                .title("Resume Incomplete Downloads?")
                .style(Style::default().fg(Color::Yellow).bg(Color::Black));
            
            frame.render_widget(popup_block, popup_area);
            
            // Render message
            let message_area = Rect {
                x: popup_area.x + 2,
                y: popup_area.y + 1,
                width: popup_area.width.saturating_sub(4),
                height: 2,
            };
            
            let message = Paragraph::new(format!(
                "Found {} incomplete download(s):\n",
                self.incomplete_downloads.len()
            ))
            .style(Style::default().fg(Color::White));
            
            frame.render_widget(message, message_area);
            
            // Render list of incomplete files (up to 5)
            let list_area = Rect {
                x: popup_area.x + 2,
                y: popup_area.y + 3,
                width: popup_area.width.saturating_sub(4),
                height: self.incomplete_downloads.len().min(5) as u16,
            };
            
            let file_lines: Vec<Line> = self.incomplete_downloads
                .iter()
                .take(5)
                .map(|metadata| {
                    let progress_pct = if metadata.total_size > 0 {
                        (metadata.downloaded_size as f64 / metadata.total_size as f64 * 100.0) as u64
                    } else {
                        0
                    };
                    Line::from(vec![
                        Span::raw("  • "),
                        Span::styled(&metadata.filename, Style::default().fg(Color::Cyan)),
                        Span::raw(format!(" ({}%)", progress_pct)),
                    ])
                })
                .collect();
            
            let files_widget = Paragraph::new(file_lines)
                .style(Style::default().fg(Color::White));
            
            frame.render_widget(files_widget, list_area);
            
            // Show "and X more..." if there are more than 5
            if self.incomplete_downloads.len() > 5 {
                let more_area = Rect {
                    x: popup_area.x + 2,
                    y: list_area.y + list_area.height,
                    width: popup_area.width.saturating_sub(4),
                    height: 1,
                };
                
                let more_text = Paragraph::new(format!("  ... and {} more", self.incomplete_downloads.len() - 5))
                    .style(Style::default().fg(Color::DarkGray));
                
                frame.render_widget(more_text, more_area);
            }
            
            // Render instructions
            let instructions_area = Rect {
                x: popup_area.x + 2,
                y: popup_area.y + popup_area.height.saturating_sub(3),
                width: popup_area.width.saturating_sub(4),
                height: 2,
            };
            
            let instructions = Paragraph::new(vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled("Y", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::raw(" to resume all  |  "),
                    Span::styled("N", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                    Span::raw(" to skip  |  "),
                    Span::styled("D", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                    Span::raw(" to delete and skip"),
                ]),
            ])
            .style(Style::default().fg(Color::White));
            
            frame.render_widget(instructions, instructions_area);
        } else if self.popup_mode == PopupMode::DownloadPath {
            // Calculate centered popup area
            let popup_width = 60.min(frame.area().width.saturating_sub(4));
            let popup_height = 7;
            let popup_x = (frame.area().width.saturating_sub(popup_width)) / 2;
            let popup_y = (frame.area().height.saturating_sub(popup_height)) / 2;
            
            let popup_area = Rect {
                x: popup_x,
                y: popup_y,
                width: popup_width,
                height: popup_height,
            };
            
            // Clear the popup area first to remove any underlying content
            frame.render_widget(Clear, popup_area);
            
            // Render popup background
            let popup_block = Block::default()
                .borders(Borders::ALL)
                .title("Download Model")
                .style(Style::default().fg(Color::White).bg(Color::Black));
            
            frame.render_widget(popup_block, popup_area);
            
            // Render input label
            let label_area = Rect {
                x: popup_area.x + 2,
                y: popup_area.y + 1,
                width: popup_area.width.saturating_sub(4),
                height: 1,
            };
            
            let label = Paragraph::new("Download path:")
                .style(Style::default().fg(Color::White));
            
            frame.render_widget(label, label_area);
            
            // Render input field
            let input_area = Rect {
                x: popup_area.x + 2,
                y: popup_area.y + 2,
                width: popup_area.width.saturating_sub(4),
                height: 1,
            };
            
            let width = input_area.width.max(3) as usize;
            let scroll = self.download_path_input.visual_scroll(width);
            
            let input_widget = Paragraph::new(self.download_path_input.value())
                .style(Style::default().fg(Color::Yellow))
                .scroll((0, scroll as u16));
            
            frame.render_widget(input_widget, input_area);
            
            // Set cursor position
            frame.set_cursor_position((
                input_area.x + ((self.download_path_input.visual_cursor()).max(scroll) - scroll) as u16,
                input_area.y,
            ));
            
            // Render instructions
            let instructions_area = Rect {
                x: popup_area.x + 2,
                y: popup_area.y + 4,
                width: popup_area.width.saturating_sub(4),
                height: 1,
            };
            
            let instructions = Paragraph::new("Press Enter to confirm, ESC to cancel")
                .style(Style::default().fg(Color::DarkGray));
            
            frame.render_widget(instructions, instructions_area);
        }
    }

    async fn handle_crossterm_events(&mut self) -> Result<()> {
        // Check for status messages from download tasks
        {
            let mut rx = self.status_rx.lock().await;
            while let Ok(msg) = rx.try_recv() {
                self.status = msg;
            }
        }
        
        let delay = tokio::time::sleep(tokio::time::Duration::from_millis(100));
        tokio::select! {
            maybe_event = self.event_stream.next().fuse() => {
                match maybe_event {
                    Some(Ok(evt)) => {
                        if let Event::Key(key) = evt {
                            if key.kind == KeyEventKind::Press {
                                self.on_key_event(key).await;
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ = delay => {
                // Timeout - just redraw
            }
        }
        Ok(())
    }

    async fn on_key_event(&mut self, key: KeyEvent) {
        self.error = None;

        // Handle popup input separately
        if self.popup_mode == PopupMode::ResumeDownload {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.resume_incomplete_downloads().await;
                    self.popup_mode = PopupMode::None;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.popup_mode = PopupMode::None;
                    self.incomplete_downloads.clear();
                    self.status = "Skipped incomplete downloads".to_string();
                }
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    self.delete_incomplete_downloads().await;
                    self.popup_mode = PopupMode::None;
                }
                _ => {}
            }
            return;
        } else if self.popup_mode == PopupMode::DownloadPath {
            match key.code {
                KeyCode::Enter => {
                    self.confirm_download().await;
                    self.popup_mode = PopupMode::None;
                }
                KeyCode::Esc => {
                    self.popup_mode = PopupMode::None;
                    self.status = "Download cancelled".to_string();
                }
                _ => {
                    self.download_path_input.handle_event(&Event::Key(key));
                }
            }
            return;
        }

        match self.input_mode {
            InputMode::Normal => match (key.modifiers, key.code) {
                (_, KeyCode::Char('q'))
                | (KeyModifiers::CONTROL, KeyCode::Char('c') | KeyCode::Char('C')) => self.quit(),
                (_, KeyCode::Char('/')) => {
                    self.input_mode = InputMode::Editing;
                    self.status = "Enter search query, press Enter to search, ESC to cancel".to_string();
                }
                (_, KeyCode::Char('d')) => {
                    if self.focused_pane == FocusedPane::Quantizations {
                        self.trigger_download();
                    }
                }
                (_, KeyCode::Tab) => {
                    self.toggle_focus();
                }
                (_, KeyCode::Down | KeyCode::Char('j')) => {
                    match self.focused_pane {
                        FocusedPane::Models => {
                            self.next();
                            self.load_quantizations().await;
                        }
                        FocusedPane::Quantizations => {
                            self.next_quant();
                        }
                    }
                }
                (_, KeyCode::Up | KeyCode::Char('k')) => {
                    match self.focused_pane {
                        FocusedPane::Models => {
                            self.previous();
                            self.load_quantizations().await;
                        }
                        FocusedPane::Quantizations => {
                            self.previous_quant();
                        }
                    }
                }
                (_, KeyCode::Enter) => {
                    match self.focused_pane {
                        FocusedPane::Models => {
                            // Switch focus to Quantizations list
                            self.toggle_focus();
                            self.show_model_details().await;
                        }
                        FocusedPane::Quantizations => {
                            self.show_quantization_details().await;
                        }
                    }
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
                    self.status = "Press '/' to search, Tab to switch lists, 'd' to download, 'q' to quit".to_string();
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

    fn toggle_focus(&mut self) {
        self.focused_pane = match self.focused_pane {
            FocusedPane::Models => {
                // When switching to quantizations, select first item if available
                let quants_len = futures::executor::block_on(async {
                    self.quantizations.lock().await.len()
                });
                if quants_len > 0 {
                    self.quant_list_state.select(Some(0));
                }
                FocusedPane::Quantizations
            }
            FocusedPane::Quantizations => FocusedPane::Models,
        };
    }

    fn next_quant(&mut self) {
        let quants_len = futures::executor::block_on(async {
            self.quantizations.lock().await.len()
        });
        
        if quants_len == 0 {
            return;
        }
        
        let i = match self.quant_list_state.selected() {
            Some(i) => {
                if i >= quants_len - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.quant_list_state.select(Some(i));
    }

    fn previous_quant(&mut self) {
        let quants_len = futures::executor::block_on(async {
            self.quantizations.lock().await.len()
        });
        
        if quants_len == 0 {
            return;
        }
        
        let i = match self.quant_list_state.selected() {
            Some(i) => {
                if i == 0 {
                    quants_len - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.quant_list_state.select(Some(i));
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
                drop(models_lock);
                
                // Load quantizations for first result
                self.load_quantizations().await;
                
                // Start background prefetch for all models
                self.start_background_prefetch();
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

    async fn show_quantization_details(&mut self) {
        let quantizations = self.quantizations.lock().await;
        if let Some(selected) = self.quant_list_state.selected() {
            if selected < quantizations.len() {
                let quant = &quantizations[selected];
                self.status = format!(
                    "Type: {} | Size: {} | File: {}",
                    quant.quant_type,
                    format_size(quant.size),
                    quant.filename
                );
            }
        }
    }

    async fn load_quantizations(&mut self) {
        let models = self.models.lock().await;
        if let Some(selected) = self.list_state.selected() {
            if selected < models.len() {
                let model_id = models[selected].id.clone();
                drop(models);
                
                // Check cache first
                let cache = self.quant_cache.lock().await;
                if let Some(cached_quants) = cache.get(&model_id) {
                    let mut quants_lock = self.quantizations.lock().await;
                    *quants_lock = cached_quants.clone();
                    drop(cache);
                    return;
                }
                drop(cache);
                
                self.loading_quants = true;
                let quantizations = self.quantizations.clone();
                let cache = self.quant_cache.clone();
                
                match fetch_model_files(&model_id).await {
                    Ok(quants) => {
                        let mut quants_lock = quantizations.lock().await;
                        *quants_lock = quants.clone();
                        self.loading_quants = false;
                        
                        // Store in cache
                        let mut cache_lock = cache.lock().await;
                        cache_lock.insert(model_id, quants);
                    }
                    Err(_) => {
                        self.loading_quants = false;
                        let mut quants_lock = quantizations.lock().await;
                        quants_lock.clear();
                    }
                }
            }
        }
    }

    fn start_background_prefetch(&self) {
        let models = self.models.clone();
        let cache = self.quant_cache.clone();
        
        tokio::spawn(async move {
            let models_lock = models.lock().await;
            let model_list = models_lock.clone();
            drop(models_lock);
            
            for model in model_list {
                // Check if already cached
                let cache_lock = cache.lock().await;
                let already_cached = cache_lock.contains_key(&model.id);
                drop(cache_lock);
                
                if !already_cached {
                    // Fetch quantization info
                    if let Ok(quants) = fetch_model_files(&model.id).await {
                        let mut cache_lock = cache.lock().await;
                        cache_lock.insert(model.id.clone(), quants);
                    }
                    
                    // Small delay to avoid overwhelming the API
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }
        });
    }

    fn quit(&mut self) {
        self.running = false;
    }
    
    fn trigger_download(&mut self) {
        let quantizations = futures::executor::block_on(async {
            self.quantizations.lock().await.clone()
        });
        
        if let Some(selected) = self.quant_list_state.selected() {
            if selected < quantizations.len() {
                self.popup_mode = PopupMode::DownloadPath;
                self.status = "Enter download path and press Enter".to_string();
            }
        }
    }
    
    async fn resume_incomplete_downloads(&mut self) {
        let count = self.incomplete_downloads.len();
        
        for metadata in &self.incomplete_downloads {
            // Queue the download to resume
            let base_path = PathBuf::from(&metadata.local_path).parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from(&metadata.local_path));
            
            let _ = self.download_tx.send((
                metadata.model_id.clone(),
                metadata.filename.clone(),
                base_path,
            ));
        }
        
        // Update queue size
        {
            let mut queue_size = self.download_queue_size.lock().await;
            *queue_size += count;
        }
        
        self.status = format!("Resuming {} incomplete download(s)", count);
        self.incomplete_downloads.clear();
    }

    async fn delete_incomplete_downloads(&mut self) {
        let mut deleted = 0;
        let mut errors = Vec::new();
        
        // Load registry
        let mut registry = {
            let reg = self.download_registry.lock().await;
            reg.clone()
        };
        
        for metadata in &self.incomplete_downloads {
            // Try to delete the actual .incomplete file
            let file_path = PathBuf::from(&metadata.local_path);
            let incomplete_path = PathBuf::from(format!("{}.incomplete", file_path.display()));
            
            match tokio::fs::remove_file(&incomplete_path).await {
                Ok(_) => deleted += 1,
                Err(e) => {
                    errors.push(format!("{}: {}", metadata.filename, e));
                }
            }
            
            // Remove from registry
            registry.downloads.retain(|d| d.url != metadata.url);
        }
        
        // Save updated registry
        Self::save_registry(&registry);
        {
            let mut reg = self.download_registry.lock().await;
            *reg = registry;
        }
        
        if errors.is_empty() {
            self.status = format!("Deleted {} incomplete file(s)", deleted);
        } else {
            self.status = format!("Deleted {} file(s), {} error(s): {}", deleted, errors.len(), errors.join(", "));
        }
        self.incomplete_downloads.clear();
    }

    async fn confirm_download(&mut self) {
        let models = self.models.lock().await.clone();
        let quantizations = self.quantizations.lock().await.clone();
        
        let model_selected = self.list_state.selected();
        let quant_selected = self.quant_list_state.selected();
        
        if let (Some(model_idx), Some(quant_idx)) = (model_selected, quant_selected) {
            if model_idx < models.len() && quant_idx < quantizations.len() {
                let model = &models[model_idx];
                let quant = &quantizations[quant_idx];
                
                let base_path = self.download_path_input.value().to_string();
                
                // Validate and sanitize the path to prevent path traversal
                let model_path = match validate_and_sanitize_path(&base_path, &model.id, &quant.filename) {
                    Ok(path) => path.parent().unwrap_or(&path).to_path_buf(),
                    Err(e) => {
                        self.error = Some(format!("Invalid path: {}", e));
                        self.status = "Download cancelled due to invalid path".to_string();
                        return;
                    }
                };
                
                // Check if this is a multi-part file (e.g., "00001-of-00005.gguf")
                let files_to_download = if let Some((current_part, total_parts)) = parse_multipart_filename(&quant.filename) {
                    // Generate all part filenames
                    let mut files = Vec::new();
                    for part in 1..=total_parts {
                        let part_filename = quant.filename.replace(
                            &format!("{:05}-of-{:05}", current_part, total_parts),
                            &format!("{:05}-of-{:05}", part, total_parts)
                        );
                        files.push(part_filename);
                    }
                    files
                } else {
                    vec![quant.filename.clone()]
                };
                
                let num_files = files_to_download.len();
                
                // Load registry and add metadata entries for all files
                let mut registry = {
                    let reg = self.download_registry.lock().await;
                    reg.clone()
                };
                
                for filename in &files_to_download {
                    // Validate each filename before processing
                    let validated_path = match validate_and_sanitize_path(&base_path, &model.id, filename) {
                        Ok(path) => path,
                        Err(e) => {
                            self.error = Some(format!("Invalid filename '{}': {}", filename, e));
                            continue;
                        }
                    };
                    
                    let url = format!("https://huggingface.co/{}/resolve/main/{}", model.id, filename);
                    let local_path_str = validated_path.to_string_lossy().to_string();
                    
                    // Only add if not already in registry
                    if !registry.downloads.iter().any(|d| d.url == url) {
                        registry.downloads.push(DownloadMetadata {
                            model_id: model.id.clone(),
                            filename: filename.clone(),
                            url: url.clone(),
                            local_path: local_path_str,
                            total_size: 0,
                            downloaded_size: 0,
                            status: DownloadStatus::Incomplete,
                        });
                    }
                }
                
                // Save registry with all new entries
                Self::save_registry(&registry);
                {
                    let mut reg = self.download_registry.lock().await;
                    *reg = registry;
                }
                
                // Increment queue size by number of files
                {
                    let mut queue_size = self.download_queue_size.lock().await;
                    *queue_size += num_files;
                }
                
                // Send all download requests
                let mut success_count = 0;
                for filename in &files_to_download {
                    if self.download_tx.send((
                        model.id.clone(),
                        filename.clone(),
                        model_path.clone(),
                    )).is_ok() {
                        success_count += 1;
                    }
                }
                
                if success_count > 0 {
                    if num_files > 1 {
                        self.status = format!("Queued {} parts of {} to {}", num_files, quant.filename, model_path.display());
                    } else {
                        self.status = format!("Starting download of {} to {}", quant.filename, model_path.display());
                    }
                } else {
                    self.error = Some("Failed to start download".to_string());
                }
                
                // Adjust queue size if some sends failed
                if success_count < num_files {
                    let mut queue_size = self.download_queue_size.lock().await;
                    *queue_size = queue_size.saturating_sub(num_files - success_count);
                }
            }
        }
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

fn format_size(bytes: u64) -> String {
    const GB: u64 = 1_073_741_824;
    const MB: u64 = 1_048_576;
    const KB: u64 = 1_024;
    
    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn sanitize_path_component(component: &str) -> Option<String> {
    // Reject path components that contain path traversal or are invalid
    if component.is_empty() 
        || component == "." 
        || component == ".." 
        || component.contains('/') 
        || component.contains('\\')
        || component.contains('\0') {
        return None;
    }
    
    // Remove any leading/trailing whitespace and dots
    let trimmed = component.trim().trim_start_matches('.').trim_end_matches('.');
    
    if trimmed.is_empty() {
        return None;
    }
    
    Some(trimmed.to_string())
}

fn validate_and_sanitize_path(base_path: &str, model_id: &str, filename: &str) -> Result<PathBuf, String> {
    // Validate base path
    let base = PathBuf::from(base_path);
    
    // Canonicalize base path if it exists, otherwise just validate it doesn't contain traversal
    let canonical_base = if base.exists() {
        base.canonicalize().map_err(|e| format!("Invalid base path: {}", e))?
    } else {
        // For non-existent paths, ensure they're absolute or under home/current dir
        if base.is_absolute() {
            base.clone()
        } else {
            std::env::current_dir()
                .map_err(|e| format!("Cannot determine current directory: {}", e))?
                .join(&base)
        }
    };
    
    // Validate and sanitize model_id (format: "author/model-name")
    let model_parts: Vec<&str> = model_id.split('/').collect();
    if model_parts.len() != 2 {
        return Err(format!("Invalid model ID format: {}", model_id));
    }
    
    let author = sanitize_path_component(model_parts[0])
        .ok_or_else(|| format!("Invalid author in model ID: {}", model_parts[0]))?;
    let model_name = sanitize_path_component(model_parts[1])
        .ok_or_else(|| format!("Invalid model name in model ID: {}", model_parts[1]))?;
    
    // Validate and sanitize filename - may contain subdirectory (e.g., "Q4_K_M/file.gguf")
    let filename_parts: Vec<&str> = filename.split('/').collect();
    let mut sanitized_filename_parts = Vec::new();
    
    for part in filename_parts {
        let sanitized = sanitize_path_component(part)
            .ok_or_else(|| format!("Invalid filename component: {}", part))?;
        sanitized_filename_parts.push(sanitized);
    }
    
    // Build the final path: base/author/model_name/[subdir/]filename
    let mut final_path = canonical_base.join(&author).join(&model_name);
    for part in sanitized_filename_parts {
        final_path = final_path.join(&part);
    }
    
    // Final safety check: ensure the resulting path is still under the base directory
    if let Ok(canonical_final) = final_path.canonicalize() {
        if !canonical_final.starts_with(&canonical_base) {
            return Err("Path traversal detected: final path escapes base directory".to_string());
        }
    } else {
        // File doesn't exist yet, check parent directories
        let mut check_path = final_path.clone();
        while let Some(parent) = check_path.parent() {
            if parent.exists() {
                if let Ok(canonical_parent) = parent.canonicalize() {
                    if !canonical_parent.starts_with(&canonical_base) {
                        return Err("Path traversal detected: parent path escapes base directory".to_string());
                    }
                }
                break;
            }
            check_path = parent.to_path_buf();
        }
    }
    
    Ok(final_path)
}

async fn fetch_model_files(model_id: &str) -> Result<Vec<QuantizationInfo>, reqwest::Error> {
    let url = format!(
        "https://huggingface.co/api/models/{}/tree/main",
        model_id
    );
    
    let response = reqwest::get(&url).await?;
    let files: Vec<ModelFile> = response.json().await?;
    
    let mut quantizations = Vec::new();
    let mut multi_part_groups: HashMap<String, Vec<ModelFile>> = HashMap::new();
    
    for file in &files {
        // Handle GGUF files in root directory
        if file.file_type == "file" && file.path.ends_with(".gguf") {
            // Check if this is a multi-part file
            if let Some((_, _)) = parse_multipart_filename(&file.path) {
                // Group multi-part files by their base name
                let base_name = get_multipart_base_name(&file.path);
                multi_part_groups.entry(base_name).or_insert_with(Vec::new).push(file.clone());
            } else {
                // Single file
                if let Some(quant_type) = extract_quantization_type(&file.path) {
                    quantizations.push(QuantizationInfo {
                        quant_type,
                        filename: file.path.clone(),
                        size: file.size,
                    });
                }
            }
        }
        // Handle subdirectories named by quantization type (e.g., Q4_K_M/, Q8_0/)
        else if file.file_type == "directory" {
            if is_quantization_directory(&file.path) {
                // Fetch files from this subdirectory
                let subdir_url = format!(
                    "https://huggingface.co/api/models/{}/tree/main/{}",
                    model_id, file.path
                );
                
                if let Ok(subdir_response) = reqwest::get(&subdir_url).await {
                    if let Ok(subdir_files) = subdir_response.json::<Vec<ModelFile>>().await {
                        // Calculate total size of all GGUF files in this directory
                        let total_size: u64 = subdir_files
                            .iter()
                            .filter(|f| f.file_type == "file" && f.path.ends_with(".gguf"))
                            .map(|f| f.size)
                            .sum();
                        
                        if total_size > 0 {
                            // Get first GGUF file as representative filename
                            let filename = subdir_files
                                .iter()
                                .find(|f| f.file_type == "file" && f.path.ends_with(".gguf"))
                                .map(|f| f.path.clone())
                                .unwrap_or_else(|| format!("{}/model.gguf", file.path));
                            
                            quantizations.push(QuantizationInfo {
                                quant_type: file.path.to_uppercase(),
                                filename,
                                size: total_size,
                            });
                        }
                    }
                }
            }
        }
    }
    
    // Process multi-part groups
    for (base_name, parts) in multi_part_groups {
        let total_size: u64 = parts.iter().map(|f| f.size).sum();
        if let Some(quant_type) = extract_quantization_type(&base_name) {
            // Use the first part's filename as representative
            let filename = parts.first().map(|f| f.path.clone()).unwrap_or(base_name);
            quantizations.push(QuantizationInfo {
                quant_type,
                filename,
                size: total_size,
            });
        }
    }
    
    // Sort by file size (largest first)
    quantizations.sort_by(|a, b| b.size.cmp(&a.size));
    
    Ok(quantizations)
}

fn get_multipart_base_name(filename: &str) -> String {
    // Extract base name from multi-part filename
    // E.g., "model-Q6_K-00003-of-00009.gguf" -> "model-Q6_K.gguf"
    // E.g., "model.Q4_K_M.gguf.part1of2" -> "model.Q4_K_M.gguf"
    
    // Handle 5-digit format: -00003-of-00009
    if let Some(multi_part_pos) = filename.rfind("-of-") {
        if let Some(part_start) = filename[..multi_part_pos].rfind('-') {
            let part_num = &filename[part_start + 1..multi_part_pos];
            if part_num.len() == 5 && part_num.chars().all(|c| c.is_ascii_digit()) {
                return format!("{}{}", &filename[..part_start], &filename[filename.rfind(".gguf").unwrap_or(filename.len())..]);
            }
        }
    }
    
    // Handle partNofM format: .part1of2, .part2of3, etc.
    if let Some(part_pos) = filename.rfind(".part") {
        // Check if it's followed by digits+of+digits
        let suffix = &filename[part_pos + 5..]; // Skip ".part"
        if let Some(of_pos) = suffix.find("of") {
            let part_num = &suffix[..of_pos];
            let total_num = &suffix[of_pos + 2..];
            if part_num.chars().all(|c| c.is_ascii_digit()) && total_num.chars().all(|c| c.is_ascii_digit()) {
                // Return filename without the .partNofM suffix
                return filename[..part_pos].to_string();
            }
        }
    }
    
    filename.to_string()
}

fn is_quantization_directory(dirname: &str) -> bool {
    // Check if directory name looks like a quantization type
    // Examples: Q4_K_M, Q8_0, Q5_K_S, IQ4_XS, BF16, etc.
    let upper = dirname.to_uppercase();
    upper.starts_with('Q') || upper.starts_with("IQ") || upper == "BF16" || upper == "FP16"
}

fn extract_quantization_type(filename: &str) -> Option<String> {
    // Extract quantization type from filenames like:
    // "model.Q4_K_M.gguf" or "llama-2-7b.Q5_0.gguf" or "Qwen3-VL-30B-Q8_K_XL.gguf"
    // "Qwen3-VL-4B-Thinking-1M-IQ4_XS.gguf" or "model-BF16.gguf"
    // "cerebras.MiniMax-M2-REAP-172B-A10B.Q6_K-00003-of-00009.gguf" (multi-part)
    // "MiniMax-M2-REAP-162B-A10B.Q4_K_M.gguf.part1of2" (multi-part)
    let name = filename;
    
    // Remove .partNofM suffix if present (must do this BEFORE removing .gguf)
    let name = if let Some(part_pos) = name.rfind(".part") {
        let suffix = &name[part_pos + 5..];
        if let Some(of_pos) = suffix.find("of") {
            let part_num = &suffix[..of_pos];
            if part_num.chars().all(|c| c.is_ascii_digit()) {
                &name[..part_pos]
            } else {
                name
            }
        } else {
            name
        }
    } else {
        name
    };
    
    // Now remove .gguf extension
    let mut name = name.trim_end_matches(".gguf");
    
    // Remove multi-part suffix if present (e.g., "-00003-of-00009")
    if let Some(multi_part_pos) = name.rfind("-of-") {
        // Find the start of the part number (should be format: -NNNNN-of-NNNNN)
        if let Some(part_start) = name[..multi_part_pos].rfind('-') {
            // Verify it looks like a part number (5 digits)
            let part_num = &name[part_start + 1..multi_part_pos];
            if part_num.len() == 5 && part_num.chars().all(|c| c.is_ascii_digit()) {
                // Remove the multi-part suffix
                name = &name[..part_start];
            }
        }
    }
    
    // Helper function to check if a string looks like a quantization type
    let is_quant_type = |s: &str| -> bool {
        let upper = s.to_uppercase();
        // Check for common quantization patterns
        // Q followed by digit (Q4, Q5, Q8, etc.)
        if upper.starts_with('Q') && upper.len() > 1 && upper.chars().nth(1).map_or(false, |c| c.is_ascii_digit()) {
            return true;
        }
        // IQ followed by digit (IQ4_XS, IQ3_M, etc.)
        if upper.starts_with("IQ") && upper.len() > 2 && upper.chars().nth(2).map_or(false, |c| c.is_ascii_digit()) {
            return true;
        }
        // MXFP followed by digit (MXFP4, MXFP6, MXFP8, etc.)
        // But not MXFP4_MOE (that should be split to MXFP4)
        if upper.starts_with("MXFP") && upper.len() > 4 && upper.chars().nth(4).map_or(false, |c| c.is_ascii_digit()) {
            // Make sure there's no underscore with additional suffix
            if !upper.contains('_') || upper.chars().nth(5).map_or(false, |c| c == '_' && upper.len() == 6) {
                return true;
            }
        }
        // Special formats
        if upper == "BF16" || upper == "FP16" || upper == "FP32" {
            return true;
        }
        false
    };
    
    // Try splitting by '.' first (handles model.Q4_K_M.gguf)
    let parts: Vec<&str> = name.split('.').collect();
    if parts.len() > 1 {
        if let Some(last_part) = parts.last() {
            if is_quant_type(last_part) {
                return Some(last_part.to_uppercase());
            }
        }
    }
    
    // If no dots, try splitting by '-' (handles Qwen3-VL-30B-Q8_K_XL.gguf and IQ4_XS)
    let parts: Vec<&str> = name.split('-').collect();
    for part in parts.iter().rev() {
        // First check if the whole part is a valid quant type (e.g., Q4_K_M, IQ4_XS)
        if is_quant_type(part) {
            return Some(part.to_uppercase());
        }
        // If not, check if it contains an underscore and the prefix is a quant type
        // This handles cases like "MXFP4_MOE" where MXFP4_MOE is not recognized as a whole,
        // but MXFP4 is the actual quantization type
        if part.contains('_') {
            let subparts: Vec<&str> = part.split('_').collect();
            if let Some(first) = subparts.first() {
                if is_quant_type(first) {
                    // Only use the prefix if it's different from checking the whole part
                    // This prevents Q4_K_M from becoming just Q4
                    return Some(first.to_uppercase());
                }
            }
        }
    }
    
    None
}

fn parse_multipart_filename(filename: &str) -> Option<(u32, u32)> {
    // Parse filenames like:
    // "Q2_K/MiniMax-M2-Q2_K-00001-of-00002.gguf" (5-digit format)
    // "MiniMax-M2-REAP-162B-A10B.Q4_K_M.gguf.part1of2" (partNofM format)
    // Returns (current_part, total_parts) if this is a multi-part file
    use regex::Regex;
    
    // Try 5-digit format first: 00001-of-00002
    if let Ok(re) = Regex::new(r"(\d{5})-of-(\d{5})") {
        if let Some(caps) = re.captures(filename) {
            let current_part = caps.get(1)?.as_str().parse::<u32>().ok()?;
            let total_parts = caps.get(2)?.as_str().parse::<u32>().ok()?;
            
            if total_parts > 1 && current_part <= total_parts {
                return Some((current_part, total_parts));
            }
        }
    }
    
    // Try partNofM format: part1of2, part2of3, etc.
    if let Ok(re) = Regex::new(r"part(\d+)of(\d+)") {
        if let Some(caps) = re.captures(filename) {
            let current_part = caps.get(1)?.as_str().parse::<u32>().ok()?;
            let total_parts = caps.get(2)?.as_str().parse::<u32>().ok()?;
            
            if total_parts > 1 && current_part <= total_parts {
                return Some((current_part, total_parts));
            }
        }
    }
    
    None
}

async fn start_download(
    model_id: String,
    filename: String,
    base_path: PathBuf,
    progress: Arc<Mutex<Option<DownloadProgress>>>,
    status_tx: mpsc::UnboundedSender<String>,
    complete_downloads: Arc<Mutex<HashMap<String, DownloadMetadata>>>,
) {
    // Validate filename to prevent path traversal
    let sanitized_filename = {
        let parts: Vec<&str> = filename.split('/').collect();
        let mut sanitized_parts = Vec::new();
        for part in parts {
            match sanitize_path_component(part) {
                Some(p) => sanitized_parts.push(p),
                None => {
                    let _ = status_tx.send(format!("Error: Invalid filename component: {}", part));
                    return;
                }
            }
        }
        sanitized_parts.join("/")
    };
    
    let url = format!("https://huggingface.co/{}/resolve/main/{}", model_id, sanitized_filename);
    
    // Create directory if it doesn't exist
    if let Err(e) = tokio::fs::create_dir_all(&base_path).await {
        let _ = status_tx.send(format!("Error: Failed to create directory: {}", e));
        return;
    }
    
    // Canonicalize base path for safety checks
    let canonical_base = match base_path.canonicalize() {
        Ok(path) => path,
        Err(e) => {
            let _ = status_tx.send(format!("Error: Cannot canonicalize base path: {}", e));
            return;
        }
    };
    
    let final_path = canonical_base.join(&sanitized_filename);
    
    // Ensure final path is still under base directory
    if let Some(parent) = final_path.parent() {
        if let Ok(canonical_final_parent) = parent.canonicalize() {
            if !canonical_final_parent.starts_with(&canonical_base) {
                let _ = status_tx.send("Error: Path traversal detected".to_string());
                return;
            }
        }
    }
    
    // Construct file paths  
    let incomplete_path = final_path.parent()
        .unwrap_or(&canonical_base)
        .join(format!("{}.incomplete", final_path.file_name().unwrap().to_string_lossy()));
    
    // Create parent directories for the file (in case filename contains subdirectories like "Q4_K_M/file.gguf")
    if let Some(parent) = final_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            let _ = status_tx.send(format!("Error: Failed to create parent directory: {}", e));
            return;
        }
    }
    if let Some(parent) = incomplete_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            let _ = status_tx.send(format!("Error: Failed to create parent directory for incomplete file: {}", e));
            return;
        }
    }
    
    // Load registry to check existing metadata
    let mut registry = App::load_registry();
    
    // Find or create metadata entry
    let metadata_entry = registry.downloads.iter_mut()
        .find(|d| d.url == url);
    
    let resume_from = if let Some(entry) = metadata_entry {
        // Check if file actually exists and size matches
        if incomplete_path.exists() {
            if let Ok(metadata) = tokio::fs::metadata(&incomplete_path).await {
                let size = metadata.len();
                entry.downloaded_size = size;
                let _ = status_tx.send(format!("Resuming {} from {} bytes", filename, size));
                size
            } else {
                0
            }
        } else {
            entry.downloaded_size
        }
    } else {
        0
    };
    
    const MAX_RETRIES: u32 = 5;
    let mut retries = MAX_RETRIES;
    let mut current_resume_from = resume_from;
    
    loop {
        match download_with_resume(
            &url,
            &incomplete_path,
            &final_path,
            current_resume_from,
            &progress,
            &model_id,
            &filename,
            &status_tx,
        ).await {
            Ok((final_size, expected_size)) => {
                // Verify the download is complete
                if final_size == expected_size && expected_size > 0 {
                    // Update registry: mark as complete
                    let mut registry = App::load_registry();
                    if let Some(entry) = registry.downloads.iter_mut().find(|d| d.url == url) {
                        entry.status = DownloadStatus::Complete;
                        entry.downloaded_size = final_size;
                        
                        // Update in-memory complete downloads map
                        let mut complete = complete_downloads.lock().await;
                        complete.insert(filename.clone(), entry.clone());
                    }
                    App::save_registry(&registry);
                    let _ = status_tx.send(format!("Download complete: {} ({} bytes)", filename, final_size));
                } else {
                    let _ = status_tx.send(format!("Warning: Download may be incomplete: {} (got {} bytes, expected {})", filename, final_size, expected_size));
                }
                break;
            }
            Err(e) if retries > 0 && is_transient_error(&e) => {
                retries -= 1;
                let _ = status_tx.send(format!("Download interrupted: {}. Retrying ({} left)...", e, retries));
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                
                // Update current position from incomplete file and save to registry
                if incomplete_path.exists() {
                    if let Ok(metadata) = tokio::fs::metadata(&incomplete_path).await {
                        current_resume_from = metadata.len();
                        
                        // Update registry
                        let mut registry = App::load_registry();
                        if let Some(entry) = registry.downloads.iter_mut().find(|d| d.url == url) {
                            entry.downloaded_size = current_resume_from;
                        }
                        App::save_registry(&registry);
                    }
                }
                continue;
            }
            Err(e) => {
                let _ = status_tx.send(format!("Error: Download failed after retries: {}", e));
                
                // Update registry with current state
                let mut registry = App::load_registry();
                if let Some(entry) = registry.downloads.iter_mut().find(|d| d.url == url) {
                    entry.status = DownloadStatus::Incomplete;
                    if incomplete_path.exists() {
                        if let Ok(metadata) = tokio::fs::metadata(&incomplete_path).await {
                            entry.downloaded_size = metadata.len();
                        }
                    }
                }
                App::save_registry(&registry);
                
                let mut prog = progress.lock().await;
                *prog = None;
                return;
            }
        }
    }
    
    // Clear progress when done
    let mut prog = progress.lock().await;
    *prog = None;
}

fn is_transient_error(e: &Box<dyn std::error::Error + Send + Sync>) -> bool {
    // Check if error is a reqwest error and if it's a timeout or connection error
    if let Some(reqwest_err) = e.downcast_ref::<reqwest::Error>() {
        return reqwest_err.is_timeout() || reqwest_err.is_connect();
    }
    false
}

async fn download_with_resume(
    url: &str,
    incomplete_path: &PathBuf,
    final_path: &PathBuf,
    resume_from: u64,
    progress: &Arc<Mutex<Option<DownloadProgress>>>,
    model_id: &str,
    filename: &str,
    _status_tx: &mpsc::UnboundedSender<String>,
) -> Result<(u64, u64), Box<dyn std::error::Error + Send + Sync>> {
    let local_path_str = final_path.to_string_lossy().to_string();
    let client = reqwest::Client::new();
    
    // Build request with Range header if resuming
    let mut request = client.get(url);
    if resume_from > 0 {
        request = request.header("Range", format!("bytes={}-", resume_from));
    }
    
    let response = request.send().await?;
    
    // Get total size from Content-Length or Content-Range
    let total_size = if let Some(content_range) = response.headers().get("content-range") {
        // Parse "bytes X-Y/Z" to get Z (total size)
        if let Ok(range_str) = content_range.to_str() {
            if let Some(total_str) = range_str.split('/').nth(1) {
                total_str.parse::<u64>().unwrap_or(0)
            } else {
                0
            }
        } else {
            0
        }
    } else {
        response.content_length().unwrap_or(0) + resume_from
    };
    
    // Update metadata entry in registry with total_size
    let mut registry = App::load_registry();
    
    if let Some(entry) = registry.downloads.iter_mut().find(|d| d.url == url) {
        // Update existing entry with total size
        entry.total_size = total_size;
        entry.downloaded_size = resume_from;
    } else {
        // Create new entry if it doesn't exist (shouldn't happen but be defensive)
        registry.downloads.push(DownloadMetadata {
            model_id: model_id.to_string(),
            filename: filename.to_string(),
            url: url.to_string(),
            local_path: local_path_str.clone(),
            total_size,
            downloaded_size: resume_from,
            status: DownloadStatus::Incomplete,
        });
    }
    
    App::save_registry(&registry);
    
    // Initialize progress
    {
        let mut prog = progress.lock().await;
        *prog = Some(DownloadProgress {
            model_id: model_id.to_string(),
            filename: filename.to_string(),
            downloaded: resume_from,
            total: total_size,
            speed_mbps: 0.0,
        });
    }
    
    // Open file in append mode
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&incomplete_path)
        .await?;
    
    let mut downloaded: u64 = resume_from;
    let mut stream = response.bytes_stream();
    
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;
    
    // For speed calculation
    let start_time = std::time::Instant::now();
    let mut last_update = start_time;
    let mut last_downloaded = resume_from;
    
    while let Some(item) = stream.next().await {
        let chunk = item?;
        
        file.write_all(&chunk).await?;
        
        downloaded += chunk.len() as u64;
        
        // Update progress and calculate speed every 500ms
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(last_update).as_secs_f64();
        
        if elapsed >= 0.5 {
            let bytes_since_last = downloaded - last_downloaded;
            let speed_mbps = (bytes_since_last as f64 / elapsed) / 1_048_576.0; // Convert to MB/s
            
            let mut prog = progress.lock().await;
            if let Some(p) = prog.as_mut() {
                p.downloaded = downloaded;
                p.speed_mbps = speed_mbps;
            }
            
            last_update = now;
            last_downloaded = downloaded;
        }
    }
    
    // Flush and sync
    file.flush().await?;
    file.sync_all().await?;
    
    // Rename to final path on successful completion
    tokio::fs::rename(incomplete_path, final_path).await?;
    
    Ok((downloaded, total_size))
}
