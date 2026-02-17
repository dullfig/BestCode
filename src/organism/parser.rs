//! YAML parser for organism configuration.
//!
//! Parses `organism.yaml` into an `Organism` struct by calling the
//! imperative API (register_listener, add_profile, etc.).

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;

use super::profile::{RetentionPolicy, SecurityProfile};
use super::{ListenerDef, Organism};

/// Top-level YAML structure.
#[derive(Debug, Deserialize)]
struct OrganismYaml {
    organism: OrganismMeta,
    #[serde(default)]
    listeners: Vec<ListenerYaml>,
    #[serde(default)]
    profiles: std::collections::HashMap<String, ProfileYaml>,
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
    #[serde(default)]
    agent: bool,
    #[serde(default)]
    peers: Vec<String>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProfileYaml {
    linux_user: String,
    listeners: ListenersSpec,
    #[serde(default)]
    journal: JournalSpec,
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

    // Register listeners
    for l in raw.listeners {
        let payload_tag = l
            .payload_class
            .rsplit('.')
            .next()
            .unwrap_or(&l.payload_class)
            .to_string();

        org.register_listener(ListenerDef {
            name: l.name,
            payload_tag,
            handler: l.handler,
            description: l.description,
            is_agent: l.agent,
            peers: l.peers,
            model: l.model,
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
}
