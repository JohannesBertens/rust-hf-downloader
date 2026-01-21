use clap::{Parser, Subcommand};

/// TUI and CLI for searching and downloading HuggingFace models
#[derive(Parser, Debug, Clone)]
#[command(name = "rust-hf-downloader")]
#[command(about = "TUI and CLI for searching and downloading HuggingFace models", long_about = None)]
#[command(version = "1.3.0")]
pub struct Cli {
    /// Run in CLI mode (no TUI)
    #[arg(long, global = true)]
    pub headless: bool,

    /// Output in JSON format
    #[arg(long, global = true)]
    pub json: bool,

    /// HuggingFace authentication token
    #[arg(long, global = true)]
    pub token: Option<String>,

    /// Dry run - show what would be done without executing
    #[arg(long, global = true)]
    pub dry_run: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    /// Search for models
    Search {
        /// Search query
        query: String,

        /// Sort field (downloads, likes, modified, name)
        #[arg(long)]
        sort: Option<String>,

        /// Minimum downloads filter
        #[arg(long)]
        min_downloads: Option<u64>,

        /// Minimum likes filter
        #[arg(long)]
        min_likes: Option<u64>,
    },

    /// Download a model
    Download {
        /// Model ID (e.g., "meta-llama/Llama-3.1-8B")
        model_id: String,

        /// Filter by quantization type (e.g., "Q4_K_M", "Q8_0")
        #[arg(long)]
        quantization: Option<String>,

        /// Download all files from the model
        #[arg(long)]
        all: bool,

        /// Output directory
        #[arg(short, long)]
        output: Option<String>,
    },

    /// List available files for a model
    List {
        /// Model ID (e.g., "meta-llama/Llama-3.1-8B")
        model_id: String,
    },

    /// Resume incomplete downloads
    Resume,
}
