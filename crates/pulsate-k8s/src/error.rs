//! Error type for the controller.

/// Errors raised while reconciling Gateway API resources.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A Kubernetes API call failed (list, patch status, finalizer update).
    #[error("kubernetes api error: {0}")]
    Kube(#[from] kube::Error),

    /// The translated Flow config failed to compile, so it was never published.
    ///
    /// Carries the rendered diagnostics. This should be rare — translation only
    /// emits validated shapes — and indicates a translation bug or an
    /// unsupported resource combination rather than user error.
    #[error("generated config rejected by pulsate-config:\n{0}")]
    Config(String),

    /// A finalizer apply/cleanup step failed.
    #[error("finalizer error: {0}")]
    Finalizer(String),
}

/// A controller result.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// Build a [`Error::Config`] from compilation diagnostics, rendering each
    /// against the generated source for a readable message.
    #[must_use]
    pub fn from_diagnostics(source: &str, diags: &[pulsate_config::Diagnostic]) -> Self {
        let src = pulsate_config::Source::new("k8s-gateway-api", source);
        let rendered = diags
            .iter()
            .map(|d| d.render(&src))
            .collect::<Vec<_>>()
            .join("\n");
        Error::Config(rendered)
    }
}
