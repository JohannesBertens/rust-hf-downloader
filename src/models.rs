use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub author: Option<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub likes: u64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(rename = "lastModified", default)]
    pub last_modified: Option<String>,
}


/// Extended model metadata from /api/models/{model_id}
#[derive(Debug, Clone, Deserialize)]
pub struct ModelMetadata {
    #[serde(rename = "id")]
    pub model_id: String,
    #[serde(default)]
    pub library_name: Option<String>,
    #[serde(default)]
    pub pipeline_tag: Option<String>,
    #[serde(default)]
    pub card_data: Option<ModelCardData>,
    #[serde(default)]
    pub siblings: Vec<RepoFile>,  // All files in the repo
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelCardData {
    #[serde(default)]
    pub base_model: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub language: Option<Vec<String>>,
    #[serde(default)]
    #[allow(dead_code)]
    pub datasets: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepoFile {
    pub rfilename: String,  // API uses 'rfilename' for relative path
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    #[allow(dead_code)]
    pub lfs: Option<LfsInfo>,  // Reuse existing LfsInfo struct
}

/// Model architecture types for visualization
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelType {
    Transformer,
    CNN,
    GPT,
    LSTM,
    RNN,
    BERT,
    T5,
    Whisper,
    StableDiffusion,
    ResNet,
    VGG,
    Inception,
    MobileNet,
    EfficientNet,
    VisionTransformer,
    GAN,
    Diffusion,
    Autoencoder,
    VAE,
    Seq2Seq,
    EncoderDecoder,
    DecisionTree,
    RandomForest,
    SVM,
    XGBoost,
    LightGBM,
    CatBoost,
    Custom,
    Unknown,
}

impl ModelType {
    /// Get display name for the model type
    pub fn display_name(self) -> &'static str {
        match self {
            ModelType::Transformer => "Transformer",
            ModelType::CNN => "CNN",
            ModelType::GPT => "GPT",
            ModelType::LSTM => "LSTM",
            ModelType::RNN => "RNN",
            ModelType::BERT => "BERT",
            ModelType::T5 => "T5",
            ModelType::Whisper => "Whisper",
            ModelType::StableDiffusion => "Stable Diffusion",
            ModelType::ResNet => "ResNet",
            ModelType::VGG => "VGG",
            ModelType::Inception => "Inception",
            ModelType::MobileNet => "MobileNet",
            ModelType::EfficientNet => "EfficientNet",
            ModelType::VisionTransformer => "Vision Transformer",
            ModelType::GAN => "GAN",
            ModelType::Diffusion => "Diffusion",
            ModelType::Autoencoder => "Autoencoder",
            ModelType::VAE => "VAE",
            ModelType::Seq2Seq => "Seq2Seq",
            ModelType::EncoderDecoder => "Encoder-Decoder",
            ModelType::DecisionTree => "Decision Tree",
            ModelType::RandomForest => "Random Forest",
            ModelType::SVM => "SVM",
            ModelType::XGBoost => "XGBoost",
            ModelType::LightGBM => "LightGBM",
            ModelType::CatBoost => "CatBoost",
            ModelType::Custom => "Custom",
            ModelType::Unknown => "Unknown",
        }
    }

    /// Get category of the model type
    pub fn category(self) -> &'static str {
        match self {
            ModelType::Transformer | ModelType::BERT | ModelType::T5 | ModelType::VisionTransformer => "Attention",
            ModelType::CNN | ModelType::ResNet | ModelType::VGG | ModelType::Inception | ModelType::MobileNet | ModelType::EfficientNet => "CNN",
            ModelType::GPT | ModelType::Whisper | ModelType::StableDiffusion | ModelType::Diffusion => "Generative",
            ModelType::LSTM | ModelType::RNN | ModelType::Seq2Seq | ModelType::EncoderDecoder => "Sequential",
            ModelType::GAN | ModelType::Autoencoder | ModelType::VAE => "Unsupervised",
            ModelType::DecisionTree | ModelType::RandomForest | ModelType::SVM | ModelType::XGBoost | ModelType::LightGBM | ModelType::CatBoost => "Traditional ML",
            ModelType::Custom => "Custom",
            ModelType::Unknown => "Unknown",
        }
    }

    /// Check if this is a transformer-based model
    pub fn is_transformer_based(self) -> bool {
        matches!(
            self,
            ModelType::Transformer | ModelType::BERT | ModelType::T5 | ModelType::GPT | ModelType::VisionTransformer | ModelType::Whisper
        )
    }

    /// Check if this is a CNN-based model
    pub fn is_cnn_based(self) -> bool {
        matches!(
            self,
            ModelType::CNN | ModelType::ResNet | ModelType::VGG | ModelType::Inception | ModelType::MobileNet | ModelType::EfficientNet
        )
    }

    /// Check if this is a generative model
    pub fn is_generative(self) -> bool {
        matches!(
            self,
            ModelType::GPT | ModelType::StableDiffusion | ModelType::Diffusion | ModelType::GAN | ModelType::VAE | ModelType::Whisper
        )
    }
}

/// Detect model type based on model ID, tags, and metadata
pub fn detect_model_type(model_id: &str, tags: &[String], library_name: Option<&str>, pipeline_tag: Option<&str>) -> ModelType {
    let model_id_lower = model_id.to_lowercase();
    let tags_lower: Vec<String> = tags.iter().map(|t| t.to_lowercase()).collect();

    // Check for specific model families
    if model_id_lower.contains("gpt") || model_id_lower.contains("gpt-") || tags_lower.contains(&"gpt".to_string()) {
        return ModelType::GPT;
    }
    if model_id_lower.contains("bert") || tags_lower.contains(&"bert".to_string()) {
        return ModelType::BERT;
    }
    if model_id_lower.contains("t5") || tags_lower.contains(&"t5".to_string()) {
        return ModelType::T5;
    }
    if model_id_lower.contains("whisper") || tags_lower.contains(&"whisper".to_string()) {
        return ModelType::Whisper;
    }
    if model_id_lower.contains("stable-diffusion") || tags_lower.contains(&"stable-diffusion".to_string()) {
        return ModelType::StableDiffusion;
    }
    if model_id_lower.contains("diffusion") || tags_lower.contains(&"diffusion".to_string()) {
        return ModelType::Diffusion;
    }
    if model_id_lower.contains("vit") || model_id_lower.contains("vision-transformer") || tags_lower.contains(&"vision-transformer".to_string()) {
        return ModelType::VisionTransformer;
    }
    if model_id_lower.contains("resnet") || tags_lower.contains(&"resnet".to_string()) {
        return ModelType::ResNet;
    }
    if model_id_lower.contains("vgg") || tags_lower.contains(&"vgg".to_string()) {
        return ModelType::VGG;
    }
    if model_id_lower.contains("inception") || tags_lower.contains(&"inception".to_string()) {
        return ModelType::Inception;
    }
    if model_id_lower.contains("mobilenet") || tags_lower.contains(&"mobilenet".to_string()) {
        return ModelType::MobileNet;
    }
    if model_id_lower.contains("efficientnet") || tags_lower.contains(&"efficientnet".to_string()) {
        return ModelType::EfficientNet;
    }
    if model_id_lower.contains("lstm") || tags_lower.contains(&"lstm".to_string()) {
        return ModelType::LSTM;
    }
    if model_id_lower.contains("rnn") || tags_lower.contains(&"rnn".to_string()) {
        return ModelType::RNN;
    }
    if model_id_lower.contains("gan") || tags_lower.contains(&"gan".to_string()) {
        return ModelType::GAN;
    }
    if model_id_lower.contains("autoencoder") || tags_lower.contains(&"autoencoder".to_string()) {
        return ModelType::Autoencoder;
    }
    if model_id_lower.contains("vae") || tags_lower.contains(&"vae".to_string()) {
        return ModelType::VAE;
    }

    // Check library names
    if let Some(lib) = library_name {
        match lib.to_lowercase().as_str() {
            "transformers" => {
                // Further refine by pipeline tag
                if let Some(pipeline) = pipeline_tag {
                    match pipeline {
                        "text-generation" => ModelType::GPT,
                        "feature-extraction" | "fill-mask" => ModelType::BERT,
                        "text2text-generation" => ModelType::T5,
                        "automatic-speech-recognition" => ModelType::Whisper,
                        "image-to-text" => ModelType::VisionTransformer,
                        _ => ModelType::Transformer,
                    }
                } else {
                    ModelType::Transformer
                }
            }
            "diffusers" => ModelType::StableDiffusion,
            "timm" => {
                // Image models from timm library
                if model_id_lower.contains("resnet") {
                    ModelType::ResNet
                } else if model_id_lower.contains("vgg") {
                    ModelType::VGG
                } else if model_id_lower.contains("efficientnet") {
                    ModelType::EfficientNet
                } else {
                    ModelType::CNN
                }
            }
            "sentence-transformers" => ModelType::Transformer,
            _ => ModelType::Unknown,
        }
    } else {
        // Check pipeline tags as fallback
        if let Some(pipeline) = pipeline_tag {
            match pipeline {
                "text-generation" => ModelType::GPT,
                "feature-extraction" | "fill-mask" => ModelType::BERT,
                "text2text-generation" => ModelType::T5,
                "automatic-speech-recognition" => ModelType::Whisper,
                "image-classification" => ModelType::CNN,
                "image-to-image" | "text-to-image" => ModelType::StableDiffusion,
                "image-to-text" => ModelType::VisionTransformer,
                _ => ModelType::Unknown,
            }
        } else {
            // Check file extensions as last resort
            if model_id_lower.contains(".safetensors") || model_id_lower.contains(".bin") {
                ModelType::Transformer
            } else if model_id_lower.contains(".onnx") {
                ModelType::CNN
            } else {
                ModelType::Unknown
            }
        }
    }
}

/// Tree node for hierarchical file display
#[derive(Debug, Clone)]
pub struct FileTreeNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub children: Vec<FileTreeNode>,
    pub expanded: bool,
    pub depth: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LfsInfo {
    pub oid: String,
    pub size: u64,
    #[serde(rename = "pointerSize")]
    pub pointer_size: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelFile {
    #[serde(rename = "type")]
    pub file_type: String,
    pub path: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub lfs: Option<LfsInfo>,
}

#[derive(Debug, Clone)]
pub struct QuantizationInfo {
    pub quant_type: String,
    pub filename: String,
    pub size: u64,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QuantizationGroup {
    pub quant_type: String,
    pub files: Vec<QuantizationInfo>,  // All files in this quantization type
    pub total_size: u64,
}

#[derive(Debug, Clone)]
pub struct ChunkProgress {
    pub chunk_id: usize,
    #[allow(dead_code)]
    pub start: u64,
    #[allow(dead_code)]
    pub end: u64,
    pub downloaded: u64,
    pub total: u64,
    pub speed_mbps: f64,
    pub is_active: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DownloadProgress {
    pub model_id: String,
    pub filename: String,
    pub downloaded: u64,
    pub total: u64,
    pub speed_mbps: f64,
    pub chunks: Vec<ChunkProgress>,
    pub verifying: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DownloadStatus {
    Incomplete,
    Complete,
    HashMismatch,
}

#[derive(Debug, Clone)]
pub enum ValidationStatus {
    Valid,
    Invalid(String),
    Pending,
}

#[derive(Debug, Clone)]
pub enum CanvasContent {
    SearchContent {
        query: String,
        suggestions: Vec<String>,
        selected_index: usize,
    },
    DownloadContent {
        path: String,
        validation_status: ValidationStatus,
    },
    OptionsContent {
        settings: AppOptions,
        focused_field: usize,
    },
    // ... other content types
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadMetadata {
    pub model_id: String,
    pub filename: String,
    pub url: String,
    pub local_path: String,
    pub total_size: u64,
    pub downloaded_size: u64,
    pub status: DownloadStatus,
    #[serde(default)]
    pub expected_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DownloadRegistry {
    pub downloads: Vec<DownloadMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupMode {
    None,
    DownloadPath,
    ResumeDownload,
    Options,
    AuthError { model_url: String },
    SearchPopup,
    // Advanced canvas features
    ModelVisualization,
    ModelComparison,
    NetworkActivity,
    PerformanceAnalytics,
    EnhancedVerification,
}

/// Filter presets for quick filter combinations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterPreset {
    NoFilters,
    Popular,
    HighlyRated,
    Recent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    #[allow(dead_code)]  // Kept for potential future use (inline editing)
    Editing,
}

/// Sort field options for model search
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SortField {
    #[default]
    Downloads,
    Likes,
    Modified,
    Name,
}

/// Sort direction (ascending or descending)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SortDirection {
    Ascending,
    #[default]
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedPane {
    Models,
    QuantizationGroups,
    QuantizationFiles,
    ModelMetadata,
    FileTree,
}

/// Model display mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelDisplayMode {
    Gguf,      // Show quantizations (current behavior)
    Standard,  // Show metadata + file tree
}

pub type QuantizationCache = HashMap<String, Vec<QuantizationGroup>>;
pub type CompleteDownloads = HashMap<String, DownloadMetadata>;

// Additional cache types for comprehensive API caching
pub type MetadataCache = HashMap<String, ModelMetadata>;
pub type FileTreeCache = HashMap<String, FileTreeNode>;
pub type SearchCache = HashMap<SearchKey, Vec<ModelInfo>>;

/// Search cache key that includes all filter parameters
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SearchKey {
    pub query: String,
    pub sort_field: SortField,
    pub sort_direction: SortDirection,
    pub min_downloads: u64,
    pub min_likes: u64,
}

/// Unified API cache container for all cached data
#[derive(Debug, Default)]
pub struct ApiCache {
    pub metadata: MetadataCache,
    pub quantizations: QuantizationCache,
    pub file_trees: FileTreeCache,
    pub searches: SearchCache,
}

/// Progress tracking for an active verification operation
#[derive(Debug, Clone)]
pub struct VerificationProgress {
    pub filename: String,
    #[allow(dead_code)]
    pub local_path: String,
    pub verified_bytes: u64,
    pub total_bytes: u64,
    pub speed_mbps: f64,
}

/// Item in the verification queue
#[derive(Debug, Clone)]
pub struct VerificationQueueItem {
    pub filename: String,
    pub local_path: String,
    pub expected_sha256: String,
    pub total_size: u64,
    #[allow(dead_code)]
    pub is_manual: bool,  // True if triggered by 'v' key, false if automatic
}

/// Application options/settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppOptions {
    // General
    pub default_directory: String,
    pub hf_token: Option<String>,
    
    // Download Settings
    pub concurrent_threads: usize,
    pub num_chunks: usize,
    pub min_chunk_size: u64,
    pub max_chunk_size: u64,
    pub max_retries: u32,
    pub download_timeout_secs: u64,
    pub retry_delay_secs: u64,
    pub progress_update_interval_ms: u64,
    
    // Verification Settings
    pub verification_on_completion: bool,
    pub concurrent_verifications: usize,
    pub verification_buffer_size: usize,
    pub verification_update_interval: usize,
    
    // UI State (not serialized)
    #[serde(skip)]
    pub selected_field: usize,
    #[serde(skip)]
    pub editing_directory: bool,
    #[serde(skip)]
    pub editing_token: bool,
    
    // Filter & Sort Settings (NEW)
    #[serde(default)]
    pub default_sort_field: SortField,
    #[serde(default)]
    pub default_sort_direction: SortDirection,
    #[serde(default)]
    pub default_min_downloads: u64,
    #[serde(default)]
    pub default_min_likes: u64,
}

impl Default for AppOptions {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let hf_token = std::env::var("HF_TOKEN").ok().filter(|s| !s.is_empty());
        Self {
            default_directory: format!("{}/models", home),
            hf_token,
            concurrent_threads: 8,
            num_chunks: 20,
            min_chunk_size: 5 * 1024 * 1024,
            max_chunk_size: 100 * 1024 * 1024,
            max_retries: 5,
            download_timeout_secs: 300,
            retry_delay_secs: 1,
            progress_update_interval_ms: 200,
            verification_on_completion: true,
            concurrent_verifications: 2,
            verification_buffer_size: 128 * 1024,
            verification_update_interval: 100,
            selected_field: 0,
            editing_directory: false,
            editing_token: false,
            // Filter & Sort defaults
            default_sort_field: SortField::Downloads,
            default_sort_direction: SortDirection::Descending,
            default_min_downloads: 0,
            default_min_likes: 0,
        }
    }
}
