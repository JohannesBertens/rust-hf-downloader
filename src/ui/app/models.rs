use super::state::App;
use crate::api::{fetch_models, fetch_model_files, fetch_trending_models};

impl App {
    /// Load trending models on startup (60 models from 2 pages)
    pub async fn load_trending_models(&mut self) {
        self.loading = true;
        self.error = None;
        
        let models = self.models.clone();
        
        match fetch_trending_models().await {
            Ok(results) => {
                let mut models_lock = models.lock().await;
                *models_lock = results;
                self.loading = false;
                self.list_state.select(Some(0));
                self.status = format!("Loaded {} trending models", models_lock.len());
                drop(models_lock);
                
                // Load quantizations for first result
                self.load_quantizations().await;
                
                // Start background prefetch for all models
                self.start_background_prefetch();
            }
            Err(e) => {
                self.loading = false;
                self.error = Some(format!("Failed to fetch trending models: {}", e));
                self.status = "Failed to load trending models".to_string();
            }
        }
    }

    /// Execute search query and load results
    pub async fn search_models(&mut self) {
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

    /// Display detailed model information in status bar
    pub async fn show_model_details(&mut self) {
        let models = self.models.lock().await;
        if let Some(selected) = self.list_state.selected() {
            if selected < models.len() {
                let model = &models[selected];
                self.selection_info = format!(
                    "Selected: {} | URL: https://huggingface.co/{}",
                    model.id, model.id
                );
            }
        }
    }

    /// Display detailed quantization information in status bar
    pub async fn show_quantization_details(&mut self) {
        let quantizations = self.quantizations.lock().await;
        if let Some(selected) = self.quant_list_state.selected() {
            if selected < quantizations.len() {
                let quant = &quantizations[selected];
                // Keep the model selection in line 1, show quant details in line 2
                self.status = format!(
                    "Type: {} | Size: {} | File: {}",
                    quant.quant_type,
                    crate::utils::format_size(quant.size),
                    quant.filename
                );
            }
        }
    }

    /// Load quantizations for currently selected model (with cache check)
    pub async fn load_quantizations(&mut self) {
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

    /// Start background task to prefetch all model quantizations
    pub fn start_background_prefetch(&self) {
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
}
