//! `pulsate-k8s` — a Kubernetes Gateway API controller for Pulsate.
//!
//! This crate watches the Gateway API resources `GatewayClass`, `Gateway`, and
//! `HTTPRoute` and continuously reconciles them into a live Pulsate configuration.
//! Each reconcile pass translates the managed resources into Pulsate Flow source
//! (see [`translate`]) and installs the result through
//! [`pulsate_config::ConfigStore::reload`] — the **same** validate-then-atomic-swap
//! path the admin reload uses (`docs/02-architecture.md#hot-reload-architecture`).
//! There is no parallel publication mechanism: a bad translation is rejected by
//! `pulsate-config` and the running snapshot is left untouched.
//!
//! ## Layout
//! - [`crd`] — minimal `kube::CustomResource` mirrors of the Gateway API `v1`
//!   types (we define them locally rather than depend on `gateway-api` so the
//!   crate pins one `kube`/`k8s-openapi` pair and stays self-contained).
//! - [`translate`] — the pure, cluster-free core: objects → Flow text.
//! - [`leader`] — the [`LeaderGuard`] gate (default [`AlwaysLeader`]).
//! - [`controller`] — the kube-runtime [`Controller`](controller::run) wiring,
//!   reconcile loop, status writeback, and finalizers.
//!
//! ## Leadership
//! Every replica runs the controller, but only the elected leader should mutate
//! config and resource status. Reconciliation is gated behind [`LeaderGuard`];
//! the default [`AlwaysLeader`] is correct for single-replica deployments. For
//! multi-replica deployments, supply a guard backed by the `pulsate-cluster`
//! gossip membership or a Kubernetes `Lease` elector.
//!
//! ## Deferred / production gates
//! - **Live-cluster testing.** The reconcile/watch wiring ([`controller::run`])
//!   needs a real apiserver and is *not* covered by the in-repo tests, which are
//!   cluster-free golden tests over [`controller::install`]. A `kind`/`envtest`
//!   integration suite (CRD apply, reconcile, status assertions) is the remaining
//!   production gate.
//! - **Multi-replica leadership.** A real [`LeaderGuard`] implementation (cluster
//!   gossip or `Lease`) must be wired before running more than one replica.
//! - **`GatewayClass` change fan-out.** Class edits are picked up on the next
//!   gateway relist rather than mapped via a reflector store.
//! - **`PathPrefix` semantics.** A Gateway API `PathPrefix` also matches the bare
//!   prefix segment; the emitted Flow glob matches sub-paths only.
#![forbid(unsafe_code)]

pub mod controller;
pub mod crd;
pub mod error;
pub mod leader;
pub mod translate;

#[doc(inline)]
pub use controller::{install, run, Context};
#[doc(inline)]
pub use crd::{Gateway, GatewayClass, HTTPRoute};
#[doc(inline)]
pub use error::{Error, Result};
#[doc(inline)]
pub use leader::{AlwaysLeader, LeaderGuard};
#[doc(inline)]
pub use translate::to_flow;

/// The default Gateway API `controllerName` this controller answers to.
///
/// A [`GatewayClass`] is managed only when its `spec.controllerName` equals this
/// value (or whatever name is passed into [`to_flow`] / [`Context`]).
pub const CONTROLLER_NAME: &str = "pulsate.io/gateway-controller";
