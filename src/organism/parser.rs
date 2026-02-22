//! YAML parser for organism configuration.
//!
//! Parses `organism.yaml` into an `Organism` struct by calling the
//! imperative API (register_listener, add_profile, etc.).

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;

use super::profile::{RetentionPolicy, SecurityProfile};
use super::{AgentConfig, ListenerDef, Organism, PortDef, WasmToolConfig};
use crate::wasm::capabilities::{EnvGrant, FsGrant, WasmCapabilities};

/// Top-level YAML structure.
#[derive(Debug, Deserialize)]
struct OrganismYaml {
    organism: OrganismMeta,
    #[serde(default)]
    listeners: Vec<ListenerYaml>,
    #[serde(default)]
    profiles: std::collections::HashMap<String, ProfileYaml>,
    #[serde(default)]
    prompts: std::collections::HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct OrganismMeta {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ListenerYaml {
    name: String,
    payload_class: String,
    handler: String,
    description: String,
    /// Supports both `agent: true` (bool) and `agent: { prompt: ... }` (config block).
    /// Also supports `is_agent: true` as an alias.
    #[serde(default, alias = "is_agent")]
    agent: AgentFieldYaml,
    #[serde(default)]
    peers: Vec<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    ports: Vec<PortYaml>,
    #[serde(default)]
    librarian: bool,
    #[serde(default)]
    wasm: Option<WasmYaml>,
    #[serde(default)]
    semantic_description: Option<String>,
}

/// Agent field: bool or config block (untagged for YAML flexibility).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AgentFieldYaml {
    Bool(bool),
    Config(AgentConfigYaml),
}

impl Default for AgentFieldYaml {
    fn default() -> Self {
        AgentFieldYaml::Bool(false)
    }
}

/// Agent configuration block parsed from YAML.
#[derive(Debug, Deserialize)]
struct AgentConfigYaml {
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    max_iterations: Option<usize>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WasmYaml {
    path: String,
    #[serde(default)]
    capabilities: Option<WasmCapabilitiesYaml>,
}

#[derive(Debug, Deserialize, Default)]
struct WasmCapabilitiesYaml {
    #[serde(default)]
    filesystem: Vec<FsGrantYaml>,
    #[serde(default)]
    env: Vec<EnvGrantYaml>,
    #[serde(default)]
    stdio: bool,
}

#[derive(Debug, Deserialize)]
struct FsGrantYaml {
    host_path: String,
    guest_path: String,
    #[serde(default)]
    read_only: bool,
}

#[derive(Debug, Deserialize)]
struct EnvGrantYaml {
    key: String,
    value: String,
}

#[derive(Debug, Deserialize)]
struct PortYaml {
    port: u16,
    direction: String,
    protocol: String,
    #[serde(default)]
    hosts: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ProfileYaml {
    linux_user: String,
    listeners: ListenersSpec,
    #[serde(default)]
    journal: JournalSpec,
    #[serde(default)]
    network: Vec<String>,
}

/// Listeners can be "all" or a list of names.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ListenersSpec {
    All(String),       // "all"
    List(Vec<String>), // ["file-ops", "shell"]
}

/// Journal retention spec.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum JournalSpec {
    Simple(String),            // "retain_forever" or "prune_on_delivery"
    WithDays(JournalDaysSpec), // { retain_days: 90 }
}

impl Default for JournalSpec {
    fn default() -> Self {
        JournalSpec::Simple("retain_forever".into())
    }
}

#[derive(Debug, Deserialize)]
struct JournalDaysSpec {
    retain_days: u16,
}

/// Load an organism from a YAML file.
pub fn load_organism(path: &Path) -> Result<Organism, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    parse_organism(&contents)
}

/// Parse an organism from a YAML string.
pub fn parse_organism(yaml: &str) -> Result<Organism, String> {
    let raw: OrganismYaml =
        serde_yaml::from_str(yaml).map_err(|e| format!("YAML parse error: {e}"))?;

    let mut org = Organism::new(&raw.organism.name);

    // Register prompts (resolve file: prefixes)
    for (name, value) in raw.prompts {
        if let Some(path) = value.strip_prefix("file:") {
            let content = crate::agent::prompts::load_prompt_file(Path::new(path.trim()))?;
            org.register_prompt(name, content);
        } else {
            org.register_prompt(name, value);
        }
    }

    // Register listeners
    for l in raw.listeners {
        let payload_tag = l
            .payload_class
            .rsplit('.')
            .next()
            .unwrap_or(&l.payload_class)
            .to_string();

        let ports = l
            .ports
            .into_iter()
            .map(|p| PortDef {
                port: p.port,
                direction: p.direction,
                protocol: p.protocol,
                hosts: p.hosts,
            })
            .collect();

        // Resolve agent field: bool or config block
        let (is_agent, agent_config) = match l.agent {
            AgentFieldYaml::Config(cfg) => {
                let config = AgentConfig {
                    prompt: cfg.prompt,
                    max_tokens: cfg.max_tokens.unwrap_or(4096),
                    max_iterations: cfg.max_iterations.unwrap_or(5),
                    model: cfg.model,
                };
                (true, Some(config))
            }
            AgentFieldYaml::Bool(b) => {
                if b {
                    (true, Some(AgentConfig::default()))
                } else {
                    (false, None)
                }
            }
        };

        org.register_listener(ListenerDef {
            name: l.name,
            payload_tag,
            handler: l.handler,
            description: l.description,
            is_agent,
            peers: l.peers,
            model: l.model,
            ports,
            librarian: l.librarian,
            semantic_description: l.semantic_description,
            agent_config,
            wasm: l.wasm.map(|w| {
                let caps = match w.capabilities {
                    Some(c) => WasmCapabilities {
                        filesystem: c
                            .filesystem
                            .into_iter()
                            .map(|f| FsGrant {
                                host_path: f.host_path,
                                guest_path: f.guest_path,
                                read_only: f.read_only,
                            })
                            .collect(),
                        env_vars: c
                            .env
                            .into_iter()
                            .map(|e| EnvGrant {
                                key: e.key,
                                value: e.value,
                            })
                            .collect(),
                        stdio: c.stdio,
                    },
                    None => WasmCapabilities::default(),
                };
                WasmToolConfig {
                    path: w.path,
                    capabilities: caps,
                }
            }),
        })?;
    }

    // Register profiles
    for (name, p) in raw.profiles {
        let (allow_all, allowed_listeners) = match p.listeners {
            ListenersSpec::All(ref s) if s == "all" => (true, HashSet::new()),
            ListenersSpec::All(ref s) => {
                // Single listener name that isn't "all"
                let mut set = HashSet::new();
                set.insert(s.clone());
                (false, set)
            }
            ListenersSpec::List(names) => (false, names.into_iter().collect()),
        };

        let journal_retention = match p.journal {
            JournalSpec::Simple(ref s) if s == "retain_forever" => RetentionPolicy::Forever,
            JournalSpec::Simple(ref s) if s == "prune_on_delivery" => {
                RetentionPolicy::PruneOnDelivery
            }
            JournalSpec::Simple(ref s) => {
                return Err(format!("unknown journal retention: '{s}'"));
            }
            JournalSpec::WithDays(spec) => RetentionPolicy::RetainDays(spec.retain_days),
        };

        org.add_profile(SecurityProfile {
            name,
            linux_user: p.linux_user,
            allowed_listeners,
            allow_all,
            journal_retention,
            network: p.network,
        })?;
    }

    Ok(org)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_organism() {
        let yaml = r#"
organism:
  name: bestcode

listeners:
  - name: coding-agent
    payload_class: handlers.code.CodeRequest
    handler: handlers.code.handle
    description: "Opus coding agent"
    agent: true
    peers: [file-ops, shell]
    model: opus

  - name: file-ops
    payload_class: handlers.files.FileRequest
    handler: handlers.files.handle
    description: "File operations"

  - name: shell
    payload_class: handlers.shell.ShellRequest
    handler: handlers.shell.handle
    description: "Shell execution"

  - name: faq
    payload_class: handlers.faq.FaqRequest
    handler: handlers.faq.handle
    description: "FAQ handler"

profiles:
  root:
    linux_user: agentos-root
    listeners: all
    journal: retain_forever
  admin:
    linux_user: agentos-admin
    listeners: [file-ops, shell, coding-agent]
    journal:
      retain_days: 90
  public:
    linux_user: agentos-public
    listeners: [faq]
    journal: prune_on_delivery
"#;

        let org = parse_organism(yaml).unwrap();
        assert_eq!(org.name, "bestcode");
        assert_eq!(org.listener_names().len(), 4);

        // Root profile allows all
        let root_table = org.dispatch_table("root").unwrap();
        assert_eq!(root_table.listeners.len(), 4);

        // Admin profile allows 3
        let admin_table = org.dispatch_table("admin").unwrap();
        assert_eq!(admin_table.listeners.len(), 3);
        assert!(admin_table.has_listener("file-ops"));
        assert!(!admin_table.has_listener("faq"));

        // Public profile allows 1
        let public_table = org.dispatch_table("public").unwrap();
        assert_eq!(public_table.listeners.len(), 1);
        assert!(public_table.has_listener("faq"));
    }

    #[test]
    fn parse_minimal_organism() {
        let yaml = r#"
organism:
  name: minimal
listeners: []
"#;
        let org = parse_organism(yaml).unwrap();
        assert_eq!(org.name, "minimal");
        assert_eq!(org.listener_names().len(), 0);
    }

    #[test]
    fn parse_organism_with_ports_and_network() {
        let yaml = r#"
organism:
  name: bestcode-m2

listeners:
  - name: llm-pool
    payload_class: llm.LlmRequest
    handler: llm.handle
    description: "LLM inference pool"
    peers: [coding-agent]
    ports:
      - port: 443
        direction: outbound
        protocol: https
        hosts: [api.anthropic.com]

  - name: file-ops
    payload_class: tools.FileOpsRequest
    handler: tools.file_ops.handle
    description: "File operations"

  - name: shell
    payload_class: tools.ShellRequest
    handler: tools.shell.handle
    description: "Shell execution"

profiles:
  admin:
    linux_user: agentos-admin
    listeners: [file-ops, shell, llm-pool]
    network: [llm-pool]
    journal:
      retain_days: 90
  public:
    linux_user: agentos-public
    listeners: [file-ops]
    journal: prune_on_delivery
"#;

        let org = parse_organism(yaml).unwrap();
        assert_eq!(org.name, "bestcode-m2");

        // LLM pool has port declarations
        let llm = org.get_listener("llm-pool").unwrap();
        assert_eq!(llm.ports.len(), 1);
        assert_eq!(llm.ports[0].port, 443);
        assert_eq!(llm.ports[0].direction, "outbound");
        assert_eq!(llm.ports[0].protocol, "https");
        assert_eq!(llm.ports[0].hosts, vec!["api.anthropic.com"]);

        // File-ops has no ports
        let fops = org.get_listener("file-ops").unwrap();
        assert!(fops.ports.is_empty());

        // Admin profile has network field
        let admin = org.get_profile("admin").unwrap();
        assert_eq!(admin.network, vec!["llm-pool"]);

        // Public profile has empty network
        let public = org.get_profile("public").unwrap();
        assert!(public.network.is_empty());
    }

    #[test]
    fn parse_librarian_flag() {
        let yaml = r#"
organism:
  name: test-librarian

listeners:
  - name: llm-pool
    payload_class: llm.LlmRequest
    handler: llm.handle
    description: "LLM pool"
    librarian: true

  - name: echo
    payload_class: handlers.echo.Greeting
    handler: handlers.echo.handle
    description: "Echo"

profiles:
  admin:
    linux_user: agentos-admin
    listeners: [llm-pool, echo]
    journal: retain_forever
"#;
        let org = parse_organism(yaml).unwrap();

        // llm-pool has librarian: true
        let llm = org.get_listener("llm-pool").unwrap();
        assert!(llm.librarian);

        // echo defaults to librarian: false
        let echo = org.get_listener("echo").unwrap();
        assert!(!echo.librarian);
    }

    #[test]
    fn parse_invalid_yaml() {
        let err = parse_organism("{{invalid").unwrap_err();
        assert!(err.contains("YAML parse error"));
    }

    #[test]
    fn profile_references_missing_listener() {
        let yaml = r#"
organism:
  name: bad

profiles:
  broken:
    linux_user: nobody
    listeners: [nonexistent]
    journal: retain_forever
"#;
        let err = parse_organism(yaml).unwrap_err();
        assert!(err.contains("unknown listener"));
    }

    // ── Phase 5: WASM listener parsing ──

    #[test]
    fn parse_wasm_listener() {
        let yaml = r#"
organism:
  name: test-wasm

listeners:
  - name: echo
    payload_class: tools.EchoRequest
    handler: wasm
    description: "Echo tool (WASM)"
    wasm:
      path: tools/echo.wasm

profiles:
  admin:
    linux_user: agentos-admin
    listeners: [echo]
    journal: retain_forever
"#;
        let org = parse_organism(yaml).unwrap();
        let echo = org.get_listener("echo").unwrap();
        assert_eq!(echo.handler, "wasm");
        let wasm = echo.wasm.as_ref().expect("wasm config should be present");
        assert_eq!(wasm.path, "tools/echo.wasm");
    }

    #[test]
    fn parse_wasm_with_capabilities() {
        let yaml = r#"
organism:
  name: test-wasm-caps

listeners:
  - name: my-tool
    payload_class: tools.MyToolRequest
    handler: wasm
    description: "My custom tool"
    wasm:
      path: tools/my_tool.wasm
      capabilities:
        filesystem:
          - host_path: /data
            guest_path: /data
            read_only: true
        env:
          - key: RUST_LOG
            value: info
        stdio: true

profiles:
  admin:
    linux_user: agentos-admin
    listeners: [my-tool]
    journal: retain_forever
"#;
        let org = parse_organism(yaml).unwrap();
        let tool = org.get_listener("my-tool").unwrap();
        let wasm = tool.wasm.as_ref().unwrap();
        assert_eq!(wasm.path, "tools/my_tool.wasm");
        assert_eq!(wasm.capabilities.filesystem.len(), 1);
        assert_eq!(wasm.capabilities.filesystem[0].host_path, "/data");
        assert!(wasm.capabilities.filesystem[0].read_only);
        assert_eq!(wasm.capabilities.env_vars.len(), 1);
        assert_eq!(wasm.capabilities.env_vars[0].key, "RUST_LOG");
        assert!(wasm.capabilities.stdio);
    }

    #[test]
    fn parse_listener_without_wasm() {
        let yaml = r#"
organism:
  name: test-no-wasm

listeners:
  - name: file-ops
    payload_class: tools.FileOpsRequest
    handler: tools.file_ops.handle
    description: "File operations"

profiles:
  admin:
    linux_user: agentos-admin
    listeners: [file-ops]
    journal: retain_forever
"#;
        let org = parse_organism(yaml).unwrap();
        let fops = org.get_listener("file-ops").unwrap();
        assert!(fops.wasm.is_none());
    }

    #[test]
    fn parse_wasm_empty_capabilities() {
        let yaml = r#"
organism:
  name: test-wasm-empty

listeners:
  - name: echo
    payload_class: tools.EchoRequest
    handler: wasm
    description: "Echo (no caps)"
    wasm:
      path: tools/echo.wasm

profiles:
  admin:
    linux_user: agentos-admin
    listeners: [echo]
    journal: retain_forever
"#;
        let org = parse_organism(yaml).unwrap();
        let echo = org.get_listener("echo").unwrap();
        let wasm = echo.wasm.as_ref().unwrap();
        assert!(wasm.capabilities.filesystem.is_empty());
        assert!(wasm.capabilities.env_vars.is_empty());
        assert!(!wasm.capabilities.stdio);
    }

    // ── Semantic Routing: semantic_description parsing ──

    #[test]
    fn parse_semantic_description_yaml() {
        let yaml = r#"
organism:
  name: test-routing

listeners:
  - name: file-ops
    payload_class: tools.FileOpsRequest
    handler: tools.file_ops.handle
    description: "File operations"
    semantic_description: |
      This tool reads, writes, and manages files on the local filesystem.
      Use it when you need to examine source code or read configuration files.

profiles:
  admin:
    linux_user: agentos-admin
    listeners: [file-ops]
    journal: retain_forever
"#;
        let org = parse_organism(yaml).unwrap();
        let fops = org.get_listener("file-ops").unwrap();
        let desc = fops.semantic_description.as_ref().unwrap();
        assert!(desc.contains("reads, writes, and manages files"));
    }

    #[test]
    fn parse_missing_semantic_description() {
        let yaml = r#"
organism:
  name: test-no-routing

listeners:
  - name: shell
    payload_class: tools.ShellRequest
    handler: tools.shell.handle
    description: "Shell execution"

profiles:
  admin:
    linux_user: agentos-admin
    listeners: [shell]
    journal: retain_forever
"#;
        let org = parse_organism(yaml).unwrap();
        let shell = org.get_listener("shell").unwrap();
        assert!(shell.semantic_description.is_none());
    }

    // ── YAML-Defined Agents: prompts section, agent config block ──

    #[test]
    fn parse_prompts_section() {
        let yaml = r#"
organism:
  name: test-prompts

prompts:
  greeting: "Hello, agent!"
  safety: |
    You are bounded.
    You do not pursue goals beyond your task.

listeners: []
"#;
        let org = parse_organism(yaml).unwrap();
        assert_eq!(org.get_prompt("greeting"), Some("Hello, agent!"));
        assert!(org.get_prompt("safety").unwrap().contains("You are bounded"));
        assert_eq!(org.prompts().len(), 2);
    }

    #[test]
    fn parse_agent_config_block() {
        let yaml = r#"
organism:
  name: test-agent-config

listeners:
  - name: coding-agent
    payload_class: agent.AgentTask
    handler: agent.handle
    description: "Coding agent"
    agent:
      prompt: "safety & coding_base"
      max_tokens: 8192
      max_iterations: 10
      model: haiku
    peers: [file-read]

profiles:
  admin:
    linux_user: agentos-admin
    listeners: [coding-agent]
    journal: retain_forever
"#;
        let org = parse_organism(yaml).unwrap();
        let agent = org.get_listener("coding-agent").unwrap();
        assert!(agent.is_agent);

        let cfg = agent.agent_config.as_ref().unwrap();
        assert_eq!(cfg.prompt.as_deref(), Some("safety & coding_base"));
        assert_eq!(cfg.max_tokens, 8192);
        assert_eq!(cfg.max_iterations, 10);
        assert_eq!(cfg.model.as_deref(), Some("haiku"));
    }

    #[test]
    fn parse_agent_bool_compat() {
        let yaml = r#"
organism:
  name: test-bool

listeners:
  - name: coding-agent
    payload_class: agent.AgentTask
    handler: agent.handle
    description: "Coding agent"
    agent: true
    peers: [file-read]

profiles:
  admin:
    linux_user: agentos-admin
    listeners: [coding-agent]
    journal: retain_forever
"#;
        let org = parse_organism(yaml).unwrap();
        let agent = org.get_listener("coding-agent").unwrap();
        assert!(agent.is_agent);

        // Bool true → default AgentConfig
        let cfg = agent.agent_config.as_ref().unwrap();
        assert_eq!(cfg.prompt, None);
        assert_eq!(cfg.max_tokens, 4096);
        assert_eq!(cfg.max_iterations, 5);
        assert_eq!(cfg.model, None);
    }

    #[test]
    fn parse_is_agent_alias() {
        let yaml = r#"
organism:
  name: test-alias

listeners:
  - name: coding-agent
    payload_class: agent.AgentTask
    handler: agent.handle
    description: "Coding agent"
    is_agent: true
    peers: [file-read]

profiles:
  admin:
    linux_user: agentos-admin
    listeners: [coding-agent]
    journal: retain_forever
"#;
        let org = parse_organism(yaml).unwrap();
        let agent = org.get_listener("coding-agent").unwrap();
        assert!(agent.is_agent);
        assert!(agent.agent_config.is_some());
    }

    #[test]
    fn parse_agent_config_defaults() {
        let yaml = r#"
organism:
  name: test-defaults

listeners:
  - name: agent
    payload_class: agent.Task
    handler: agent.handle
    description: "Agent"
    agent:
      prompt: "my_prompt"

profiles:
  admin:
    linux_user: agentos-admin
    listeners: [agent]
    journal: retain_forever
"#;
        let org = parse_organism(yaml).unwrap();
        let agent = org.get_listener("agent").unwrap();
        assert!(agent.is_agent);

        let cfg = agent.agent_config.as_ref().unwrap();
        assert_eq!(cfg.prompt.as_deref(), Some("my_prompt"));
        // Defaults
        assert_eq!(cfg.max_tokens, 4096);
        assert_eq!(cfg.max_iterations, 5);
        assert_eq!(cfg.model, None);
    }

    #[test]
    fn parse_agent_false_no_config() {
        let yaml = r#"
organism:
  name: test-false

listeners:
  - name: tool
    payload_class: tools.Request
    handler: tools.handle
    description: "A tool"

profiles:
  admin:
    linux_user: agentos-admin
    listeners: [tool]
    journal: retain_forever
"#;
        let org = parse_organism(yaml).unwrap();
        let tool = org.get_listener("tool").unwrap();
        assert!(!tool.is_agent);
        assert!(tool.agent_config.is_none());
    }

    #[test]
    fn parse_file_prompt() {
        let dir = tempfile::TempDir::new().unwrap();
        let prompt_path = dir.path().join("test_prompt.md");
        std::fs::write(&prompt_path, "You are a test prompt from a file.").unwrap();

        // Use forward slashes for YAML compatibility (avoids hex escape issues)
        let path_str = prompt_path.display().to_string().replace('\\', "/");
        let yaml = format!(
            r#"
organism:
  name: test-file-prompt

prompts:
  from_file: "file:{path_str}"

listeners: []
"#,
        );

        let org = parse_organism(&yaml).unwrap();
        assert_eq!(
            org.get_prompt("from_file"),
            Some("You are a test prompt from a file.")
        );
    }
}
