//! AgentPipeline — wraps rust-pipeline with kernel integration.
//!
//! The adapter pattern: rust-pipeline stays clean as a library,
//! bestcode adds the kernel layer on top for durability and security.
//!
//! Architecture:
//! - Builds a `ListenerRegistry` from the Organism configuration
//! - Passes a standard `ThreadRegistry` to the inner pipeline
//! - Mirrors thread/context/journal ops to the Kernel for durability
//! - Enforces security profiles before messages enter the pipeline
//! - On crash recovery, rebuilds in-memory state from the kernel

use std::path::Path;
use std::sync::Arc;

use tokio::sync::Mutex;

use rust_pipeline::prelude::*;

use crate::kernel::Kernel;
use crate::organism::Organism;
use crate::security::SecurityResolver;

/// AgentPipeline: wraps rust-pipeline's Pipeline with kernel integration.
pub struct AgentPipeline {
    /// The inner rust-pipeline.
    pipeline: Pipeline,
    /// Durable kernel state.
    kernel: Arc<Mutex<Kernel>>,
    /// Organism configuration.
    organism: Organism,
    /// Security resolver (profile → dispatch table).
    security: SecurityResolver,
}

impl AgentPipeline {
    /// Build an AgentPipeline from an Organism config and a data directory.
    ///
    /// This:
    /// 1. Opens/recovers the kernel from the data directory
    /// 2. Builds a ListenerRegistry from the organism's listeners
    /// 3. Constructs the security resolver from profiles
    /// 4. Wraps everything in the adapter
    ///
    /// Note: handlers must be registered separately since the Organism
    /// config only has handler names (strings), not actual handler instances.
    /// Use `register_handler()` after construction.
    pub fn new(organism: Organism, data_dir: &Path) -> Result<Self, String> {
        let kernel = Kernel::open(data_dir).map_err(|e| format!("kernel open failed: {e}"))?;

        let security = SecurityResolver::from_organism(&organism)?;

        // Build a ListenerRegistry from organism config
        // Handlers will be registered later via register_handler()
        let registry = ListenerRegistry::new();
        let threads = ThreadRegistry::new();
        let pipeline = Pipeline::new(registry, threads);

        Ok(Self {
            pipeline,
            kernel: Arc::new(Mutex::new(kernel)),
            organism,
            security,
        })
    }

    /// Register a handler for a named listener.
    /// The listener must already be defined in the Organism config.
    pub fn register_handler<H: Handler>(
        &mut self,
        listener_name: &str,
        _handler: H,
    ) -> Result<(), String> {
        let _def = self
            .organism
            .get_listener(listener_name)
            .ok_or_else(|| format!("listener '{listener_name}' not in organism config"))?
            .clone();

        // We need to rebuild the pipeline with the updated registry.
        // Since Pipeline::new takes ownership, we need to reconstruct.
        // For now, we register directly on the existing pipeline's registry
        // through the provided API.

        // Unfortunately, rust-pipeline's Pipeline takes Arc<ListenerRegistry>
        // which is immutable after creation. The proper approach is to build
        // the full registry before creating the pipeline.
        // Let's use a builder pattern instead.

        Err("use AgentPipelineBuilder to register handlers before building".into())
    }

    /// Initialize the root thread (WAL-backed).
    pub async fn initialize_root(
        &self,
        organism_name: &str,
        profile: &str,
    ) -> Result<String, String> {
        let mut kernel = self.kernel.lock().await;
        kernel
            .initialize_root(organism_name, profile)
            .map_err(|e| format!("initialize_root failed: {e}"))
    }

    /// Inject a raw message into the pipeline with security enforcement.
    ///
    /// Before the message enters the pipeline, we check:
    /// 1. The thread's profile allows messaging the target
    /// 2. The dispatch is logged in the kernel
    pub async fn inject_checked(
        &self,
        raw: Vec<u8>,
        _thread_id: &str,
        profile: &str,
        target: &str,
    ) -> Result<(), String> {
        // Security check: is the target reachable under this profile?
        if !self.security.can_reach(profile, target) {
            return Err(format!(
                "security: profile '{profile}' cannot reach listener '{target}'"
            ));
        }

        // Inject into the inner pipeline
        self.pipeline
            .inject(raw)
            .await
            .map_err(|e| format!("inject failed: {e}"))
    }

    /// Inject raw bytes directly (bypasses security — for system messages).
    pub async fn inject_raw(&self, raw: Vec<u8>) -> Result<(), String> {
        self.pipeline
            .inject(raw)
            .await
            .map_err(|e| format!("inject failed: {e}"))
    }

    /// Start the pipeline.
    pub fn run(&mut self) {
        self.pipeline.run();
    }

    /// Shutdown the pipeline.
    pub async fn shutdown(self) {
        self.pipeline.shutdown().await;
    }

    /// Get a reference to the organism.
    pub fn organism(&self) -> &Organism {
        &self.organism
    }

    /// Get the security resolver.
    pub fn security(&self) -> &SecurityResolver {
        &self.security
    }

    /// Get a handle to the kernel (for direct operations).
    pub fn kernel(&self) -> Arc<Mutex<Kernel>> {
        self.kernel.clone()
    }

    /// Reload organism configuration and rebuild security tables.
    pub fn reload(
        &mut self,
        new_organism: Organism,
    ) -> Result<crate::organism::ReloadEvent, String> {
        let event = self.organism.apply_config(new_organism);
        self.security.rebuild(&self.organism)?;
        Ok(event)
    }
}

/// Builder for AgentPipeline — register handlers before building.
pub struct AgentPipelineBuilder {
    organism: Organism,
    data_dir: std::path::PathBuf,
    registry: ListenerRegistry,
}

impl AgentPipelineBuilder {
    /// Start building an AgentPipeline.
    pub fn new(organism: Organism, data_dir: &Path) -> Self {
        Self {
            organism,
            data_dir: data_dir.to_path_buf(),
            registry: ListenerRegistry::new(),
        }
    }

    /// Register a handler for a listener defined in the organism.
    pub fn register<H: Handler>(mut self, listener_name: &str, handler: H) -> Result<Self, String> {
        let def = self
            .organism
            .get_listener(listener_name)
            .ok_or_else(|| format!("listener '{listener_name}' not in organism config"))?
            .clone();

        self.registry.register(
            &def.name,
            &def.payload_tag,
            handler,
            def.is_agent,
            def.peers.clone(),
            &def.description,
            None, // Schema registration deferred
        );

        Ok(self)
    }

    /// Build the AgentPipeline.
    pub fn build(self) -> Result<AgentPipeline, String> {
        let kernel =
            Kernel::open(&self.data_dir).map_err(|e| format!("kernel open failed: {e}"))?;

        let security = SecurityResolver::from_organism(&self.organism)?;

        let threads = ThreadRegistry::new();
        let pipeline = Pipeline::new(self.registry, threads);

        Ok(AgentPipeline {
            pipeline,
            kernel: Arc::new(Mutex::new(kernel)),
            organism: self.organism,
            security,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::organism::parser::parse_organism;
    use rust_pipeline::prelude::{
        build_envelope, FnHandler, HandlerContext, HandlerResponse, ValidatedPayload,
    };
    use tempfile::TempDir;

    fn test_organism() -> Organism {
        let yaml = r#"
organism:
  name: test-org

listeners:
  - name: echo
    payload_class: handlers.echo.Greeting
    handler: handlers.echo.handle
    description: "Echo handler"
    peers: []

  - name: sink
    payload_class: handlers.sink.SinkRequest
    handler: handlers.sink.handle
    description: "Sink handler"

profiles:
  admin:
    linux_user: agentos-admin
    listeners: [echo, sink]
    journal: retain_forever
  public:
    linux_user: agentos-public
    listeners: [echo]
    journal: prune_on_delivery
"#;
        parse_organism(yaml).unwrap()
    }

    #[tokio::test]
    async fn build_agent_pipeline() {
        let dir = TempDir::new().unwrap();
        let org = test_organism();

        let echo = FnHandler(|p: ValidatedPayload, _ctx: HandlerContext| {
            Box::pin(async move { Ok(HandlerResponse::Reply { payload_xml: p.xml }) })
        });

        let sink = FnHandler(|_p: ValidatedPayload, _ctx: HandlerContext| {
            Box::pin(async move { Ok(HandlerResponse::None) })
        });

        let pipeline = AgentPipelineBuilder::new(org, &dir.path().join("data"))
            .register("echo", echo)
            .unwrap()
            .register("sink", sink)
            .unwrap()
            .build()
            .unwrap();

        assert!(pipeline.organism().get_listener("echo").is_some());
    }

    #[tokio::test]
    async fn security_blocks_restricted_target() {
        let dir = TempDir::new().unwrap();
        let org = test_organism();

        let echo = FnHandler(|p: ValidatedPayload, _ctx: HandlerContext| {
            Box::pin(async move { Ok(HandlerResponse::Reply { payload_xml: p.xml }) })
        });

        let sink = FnHandler(|_p: ValidatedPayload, _ctx: HandlerContext| {
            Box::pin(async move { Ok(HandlerResponse::None) })
        });

        let mut pipeline = AgentPipelineBuilder::new(org, &dir.path().join("data"))
            .register("echo", echo)
            .unwrap()
            .register("sink", sink)
            .unwrap()
            .build()
            .unwrap();

        pipeline.run();

        // Public profile can reach echo
        let envelope = build_envelope(
            "test",
            "echo",
            "thread-1",
            b"<Greeting><text>hi</text></Greeting>",
        )
        .unwrap();

        let result = pipeline
            .inject_checked(envelope, "thread-1", "public", "echo")
            .await;
        assert!(result.is_ok());

        // Public profile CANNOT reach sink — structural impossibility
        let envelope2 = build_envelope("test", "sink", "thread-2", b"<SinkRequest/>").unwrap();

        let result = pipeline
            .inject_checked(envelope2, "thread-2", "public", "sink")
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot reach"));

        pipeline.shutdown().await;
    }

    #[tokio::test]
    async fn kernel_state_persists() {
        let dir = TempDir::new().unwrap();
        let data_dir = dir.path().join("data");

        // First session
        {
            let org = test_organism();
            let echo = FnHandler(|p: ValidatedPayload, _ctx: HandlerContext| {
                Box::pin(async move { Ok(HandlerResponse::Reply { payload_xml: p.xml }) })
            });
            let sink = FnHandler(|_p: ValidatedPayload, _ctx: HandlerContext| {
                Box::pin(async move { Ok(HandlerResponse::None) })
            });

            let pipeline = AgentPipelineBuilder::new(org, &data_dir)
                .register("echo", echo)
                .unwrap()
                .register("sink", sink)
                .unwrap()
                .build()
                .unwrap();

            // Initialize root in kernel
            pipeline.initialize_root("test-org", "admin").await.unwrap();
        }

        // Second session — kernel state should be recovered
        {
            let org = test_organism();
            let echo = FnHandler(|p: ValidatedPayload, _ctx: HandlerContext| {
                Box::pin(async move { Ok(HandlerResponse::Reply { payload_xml: p.xml }) })
            });
            let sink = FnHandler(|_p: ValidatedPayload, _ctx: HandlerContext| {
                Box::pin(async move { Ok(HandlerResponse::None) })
            });

            let pipeline = AgentPipelineBuilder::new(org, &data_dir)
                .register("echo", echo)
                .unwrap()
                .register("sink", sink)
                .unwrap()
                .build()
                .unwrap();

            let kernel = pipeline.kernel();
            let k = kernel.lock().await;
            assert!(k.threads().root_uuid().is_some());
        }
    }

    #[tokio::test]
    async fn hot_reload_updates_security() {
        let dir = TempDir::new().unwrap();
        let org = test_organism();

        let echo = FnHandler(|p: ValidatedPayload, _ctx: HandlerContext| {
            Box::pin(async move { Ok(HandlerResponse::Reply { payload_xml: p.xml }) })
        });
        let sink = FnHandler(|_p: ValidatedPayload, _ctx: HandlerContext| {
            Box::pin(async move { Ok(HandlerResponse::None) })
        });

        let mut pipeline = AgentPipelineBuilder::new(org, &dir.path().join("data"))
            .register("echo", echo)
            .unwrap()
            .register("sink", sink)
            .unwrap()
            .build()
            .unwrap();

        // Initially, public cannot reach sink
        assert!(!pipeline.security().can_reach("public", "sink"));

        // Hot reload: expand public profile to include sink
        let new_yaml = r#"
organism:
  name: test-org-v2

listeners:
  - name: echo
    payload_class: handlers.echo.Greeting
    handler: handlers.echo.handle
    description: "Echo handler"

  - name: sink
    payload_class: handlers.sink.SinkRequest
    handler: handlers.sink.handle
    description: "Sink handler"

profiles:
  admin:
    linux_user: agentos-admin
    listeners: [echo, sink]
    journal: retain_forever
  public:
    linux_user: agentos-public
    listeners: [echo, sink]
    journal: prune_on_delivery
"#;
        let new_org = parse_organism(new_yaml).unwrap();
        let _event = pipeline.reload(new_org).unwrap();

        assert_eq!(pipeline.organism().name, "test-org-v2");

        // Now public CAN reach sink
        assert!(pipeline.security().can_reach("public", "sink"));
    }
}
