use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use tracing::info;

use bestcode::llm::LlmPool;
use bestcode::organism::parser::parse_organism;
use bestcode::pipeline::AgentPipelineBuilder;
use bestcode::tools::{
    command_exec::CommandExecTool, file_edit::FileEditTool, file_read::FileReadTool,
    file_write::FileWriteTool, glob_tool::GlobTool, grep::GrepTool,
};
use bestcode::tui::runner::run_tui;

/// Default organism configuration embedded in the binary.
const DEFAULT_ORGANISM: &str = r#"
organism:
  name: bestcode

prompts:
  coding_base: |
    You are a coding agent running inside AgentOS. You have access to tools for file operations,
    shell commands, and codebase indexing. Use these tools to complete the task you've been given.

    Rules:
    1. Read before you write. Always understand existing code before modifying it.
    2. Make the smallest change that solves the problem.
    3. Test your changes when possible (run tests, verify output).
    4. If a tool call fails, analyze the error and try a different approach.
    5. When done, provide a clear summary of what you did.

    {tool_definitions}

  no_paperclipper: |
    You are bounded. You do not pursue goals beyond your task.
    You report uncertainty rather than improvising.

listeners:
  - name: coding-agent
    payload_class: agent.AgentTask
    handler: agent.handle
    description: "AI coding agent"
    agent:
      prompt: "no_paperclipper & coding_base"
      max_tokens: 4096
    librarian: true
    peers: [file-read, file-write, file-edit, glob, grep, command-exec, codebase-index]

  - name: llm-pool
    payload_class: llm.LlmRequest
    handler: llm.handle
    description: "LLM inference pool"
    librarian: true
    ports:
      - port: 443
        direction: outbound
        protocol: https
        hosts: [api.anthropic.com]

  - name: librarian
    payload_class: librarian.LibrarianRequest
    handler: librarian.handle
    description: "Context curator"
    peers: [llm-pool]

  - name: codebase-index
    payload_class: treesitter.CodeIndexRequest
    handler: treesitter.handle
    description: "Tree-sitter code indexing"

  - name: file-read
    payload_class: tools.FileReadRequest
    handler: tools.file_read.handle
    description: "Read files"

  - name: file-write
    payload_class: tools.FileWriteRequest
    handler: tools.file_write.handle
    description: "Write files"

  - name: file-edit
    payload_class: tools.FileEditRequest
    handler: tools.file_edit.handle
    description: "Edit files"

  - name: glob
    payload_class: tools.GlobRequest
    handler: tools.glob.handle
    description: "Glob search"

  - name: grep
    payload_class: tools.GrepRequest
    handler: tools.grep.handle
    description: "Grep search"

  - name: command-exec
    payload_class: tools.CommandExecRequest
    handler: tools.command_exec.handle
    description: "Command execution"

profiles:
  coding:
    linux_user: bestcode
    listeners: [coding-agent, file-read, file-write, file-edit, glob, grep, command-exec, codebase-index, llm-pool, librarian]
    network: [llm-pool]
    journal: retain_forever
"#;

/// Extension trait to convert Result<T, String> to anyhow::Result<T>.
trait ToAnyhow<T> {
    fn to_anyhow(self) -> Result<T>;
}

impl<T> ToAnyhow<T> for std::result::Result<T, String> {
    fn to_anyhow(self) -> Result<T> {
        self.map_err(|e| anyhow::anyhow!("{e}"))
    }
}

#[derive(Parser)]
#[command(name = "bestcode", about = "AI coding agent. No compaction, ever.")]
struct Cli {
    /// Working directory (defaults to current)
    #[arg(short, long)]
    dir: Option<String>,

    /// Model to use (default: claude-sonnet-4-20250514)
    #[arg(short, long)]
    model: Option<String>,

    /// Path to organism.yaml (default: embedded)
    #[arg(short, long)]
    organism: Option<String>,

    /// Kernel data directory (default: .bestcode/)
    #[arg(long)]
    data: Option<String>,

    /// Enable debug tab (activity trace, diagnostics)
    #[arg(long)]
    debug: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI
    let cli = Cli::parse();
    let debug = cli.debug;
    let work_dir = cli.dir.unwrap_or_else(|| ".".into());
    let model = cli
        .model
        .unwrap_or_else(|| "claude-sonnet-4-20250514".into());
    let data_rel = cli.data.unwrap_or_else(|| ".bestcode".into());
    let data_dir = PathBuf::from(&work_dir).join(&data_rel);

    // Set working directory
    std::env::set_current_dir(&work_dir)?;

    // Initialize tracing to file (avoid polluting the TUI)
    let log_dir = PathBuf::from(&data_rel);
    std::fs::create_dir_all(&log_dir)?;
    let log_file = std::fs::File::create(log_dir.join("bestcode.log"))?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("bestcode=info".parse()?),
        )
        .with_writer(log_file)
        .with_ansi(false)
        .init();

    info!("BestCode starting in {work_dir}");

    // Parse organism config
    let yaml = if let Some(ref path) = cli.organism {
        std::fs::read_to_string(path)?
    } else {
        DEFAULT_ORGANISM.to_string()
    };
    let org = parse_organism(&yaml).to_anyhow()?;

    // Create LLM pool from environment
    let pool = LlmPool::from_env(&model).map_err(|e| anyhow::anyhow!("{e}"))?;

    info!("Building pipeline with model {model}");

    // Build pipeline
    let mut pipeline = AgentPipelineBuilder::new(org, &data_dir)
        .with_llm_pool(pool)
        .to_anyhow()?
        .with_librarian()
        .to_anyhow()?
        .with_code_index()
        .to_anyhow()?
        .register("file-read", FileReadTool)
        .to_anyhow()?
        .register("file-write", FileWriteTool)
        .to_anyhow()?
        .register("file-edit", FileEditTool)
        .to_anyhow()?
        .register("glob", GlobTool)
        .to_anyhow()?
        .register("grep", GrepTool)
        .to_anyhow()?
        .register("command-exec", CommandExecTool::new())
        .to_anyhow()?
        .with_agents()
        .to_anyhow()?
        .build()
        .to_anyhow()?;

    // Initialize root thread
    pipeline
        .initialize_root("bestcode", "coding")
        .await
        .to_anyhow()?;

    info!("Pipeline ready, starting TUI");

    // Start pipeline
    pipeline.run();

    // Run TUI (blocks until quit)
    run_tui(&pipeline, debug).await?;

    // Shutdown
    info!("Shutting down");
    pipeline.shutdown().await;

    Ok(())
}
