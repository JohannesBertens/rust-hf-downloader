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
use std::collections::HashMap;
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
        
        Self {
            running: false,
            event_stream: EventStream::default(),
            input: Input::default(),
            input_mode: InputMode::Normal,
            focused_pane: FocusedPane::Models,
            models: Arc::new(Mutex::new(Vec::new())),
            list_state,
            quant_list_state,
            loading: false,
            error: None,
            status: "Press '/' to search, Tab to switch lists, 'q' to quit".to_string(),
            quantizations: Arc::new(Mutex::new(Vec::new())),
            loading_quants: false,
            quant_cache: Arc::new(Mutex::new(HashMap::new())),
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
                let content = Line::from(vec![
                    Span::raw(format!("{:>10}  ", size_str)),
                    Span::styled(
                        format!("{:<14} ", q.quant_type),
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(&q.filename, Style::default().fg(Color::DarkGray)),
                ]);
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

async fn fetch_model_files(model_id: &str) -> Result<Vec<QuantizationInfo>, reqwest::Error> {
    let url = format!(
        "https://huggingface.co/api/models/{}/tree/main",
        model_id
    );
    
    let response = reqwest::get(&url).await?;
    let files: Vec<ModelFile> = response.json().await?;
    
    let mut quantizations = Vec::new();
    
    for file in &files {
        // Handle GGUF files in root directory
        if file.file_type == "file" && file.path.ends_with(".gguf") {
            if let Some(quant_type) = extract_quantization_type(&file.path) {
                quantizations.push(QuantizationInfo {
                    quant_type,
                    filename: file.path.clone(),
                    size: file.size,
                });
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
    
    // Sort by file size (largest first)
    quantizations.sort_by(|a, b| b.size.cmp(&a.size));
    
    Ok(quantizations)
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
    let name = filename.trim_end_matches(".gguf");
    
    // Try splitting by '.' first (handles model.Q4_K_M.gguf)
    let parts: Vec<&str> = name.split('.').collect();
    if parts.len() > 1 {
        if let Some(last_part) = parts.last() {
            if last_part.starts_with('Q') || last_part.starts_with('q') {
                return Some(last_part.to_uppercase());
            }
        }
    }
    
    // If no dots, try splitting by '-' (handles Qwen3-VL-30B-Q8_K_XL.gguf)
    let parts: Vec<&str> = name.split('-').collect();
    for part in parts.iter().rev() {
        if part.starts_with('Q') || part.starts_with('q') {
            return Some(part.to_uppercase());
        }
    }
    
    None
}


