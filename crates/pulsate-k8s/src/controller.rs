//! The kube-runtime controller: watches Gateway API resources and reconciles
//! them into a live [`pulsate_config::ConfigStore`] generation.
//!
//! ## Shape
//! The [`Gateway`] is the primary reconciled object — config is cluster-global,
//! and gateways are its natural anchor. [`HTTPRoute`] changes are mapped to their
//! parent gateways via `parentRefs` (a synchronous, exact mapping) so a route
//! edit re-reconciles the right gateways. [`GatewayClass`] is watched too.
//!
//! Every reconcile pass rebuilds the **entire** config from a fresh list of all
//! managed resources and installs it through [`pulsate_config::ConfigStore::reload`]
//! — the same validate-then-atomic-swap path the admin reload uses — so the
//! triggering object's identity never changes the result. Status `Accepted`
//! conditions are written back and a finalizer guarantees a rebuild-without-it
//! when a gateway is deleted.
//!
//! Reconciliation is gated behind a [`LeaderGuard`]; non-leaders no-op and
//! requeue (see [`crate::leader`]).

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use kube::api::{ListParams, Patch, PatchParams};
use kube::runtime::controller::{Action, Controller};
use kube::runtime::finalizer::{finalizer, Event as FinalizerEvent};
use kube::runtime::reflector::ObjectRef;
use kube::runtime::watcher;
use kube::{Api, Client, ResourceExt};
use pulsate_config::ConfigStore;

use crate::crd::{Gateway, GatewayClass, HTTPRoute};
use crate::error::{Error, Result};
use crate::leader::LeaderGuard;
use crate::translate::to_flow;

/// The Flow config name used for every reconcile-driven reload (mirrors the
/// admin reload's `"admin"` name).
const CONFIG_NAME: &str = "k8s-gateway-api";

/// Finalizer key placed on managed gateways.
const FINALIZER: &str = "gateway.pulsate.io/finalizer";

/// How long to wait before a steady-state re-reconcile.
const REQUEUE: Duration = Duration::from_secs(300);

/// Shared reconcile context: the cluster client, the config store to publish
/// into, the leadership gate, and the controller name we answer to.
pub struct Context<G: LeaderGuard> {
    /// The Kubernetes API client.
    pub client: Client,
    /// The live config store published into on every reconcile.
    pub store: Arc<ConfigStore>,
    /// Leadership gate; non-leaders skip mutation.
    pub guard: G,
    /// The `controllerName` this instance manages (matches `GatewayClass`es).
    pub controller_name: String,
}

/// Translate the given resources and publish the result into `store` via the
/// shared atomic-swap reload path.
///
/// This is the cluster-free core the golden tests exercise: hand it in-memory
/// objects and a store, then inspect [`ConfigStore::current`].
///
/// # Errors
/// Returns [`Error::Config`] if the translated Flow text fails to compile (the
/// running snapshot is left untouched — validation is total before publish).
pub fn install(
    store: &ConfigStore,
    classes: &[GatewayClass],
    gateways: &[Gateway],
    routes: &[HTTPRoute],
    controller_name: &str,
) -> Result<()> {
    let text = to_flow(classes, gateways, routes, controller_name);
    store
        .reload(CONFIG_NAME, &text)
        .map(|_| ())
        .map_err(|diags| Error::from_diagnostics(&text, &diags))
}

/// List every managed resource and rebuild the config, optionally excluding one
/// gateway by `(namespace, name)` — used during finalizer cleanup so a deleted
/// gateway is gone from the published config immediately.
async fn rebuild<G: LeaderGuard>(
    ctx: &Context<G>,
    exclude_gateway: Option<(&str, &str)>,
) -> Result<()> {
    let classes: Vec<GatewayClass> = Api::all(ctx.client.clone())
        .list(&ListParams::default())
        .await?
        .items;
    let mut gateways: Vec<Gateway> = Api::all(ctx.client.clone())
        .list(&ListParams::default())
        .await?
        .items;
    if let Some((ns, name)) = exclude_gateway {
        gateways.retain(|g| !(g.namespace().as_deref() == Some(ns) && g.name_any() == name));
    }
    let routes: Vec<HTTPRoute> = Api::all(ctx.client.clone())
        .list(&ListParams::default())
        .await?
        .items;

    install(
        &ctx.store,
        &classes,
        &gateways,
        &routes,
        &ctx.controller_name,
    )
}

/// Patch an `Accepted=True` condition onto a gateway's status.
async fn mark_accepted<G: LeaderGuard>(ctx: &Context<G>, gw: &Gateway) -> Result<()> {
    let Some(ns) = gw.namespace() else {
        return Ok(());
    };
    let api: Api<Gateway> = Api::namespaced(ctx.client.clone(), &ns);
    let patch = serde_json::json!({
        "status": {
            "conditions": [{
                "type": "Accepted",
                "status": "True",
                "reason": "Reconciled",
                "message": "Gateway reconciled into the Pulsate config",
                "lastTransitionTime": k8s_openapi::chrono::Utc::now().to_rfc3339(),
                "observedGeneration": gw.metadata.generation,
            }]
        }
    });
    api.patch_status(
        &gw.name_any(),
        &PatchParams::default(),
        &Patch::Merge(&patch),
    )
    .await?;
    Ok(())
}

/// Reconcile a single [`Gateway`]: rebuild the whole config, write status, and
/// keep the finalizer in sync. Gated behind the leader guard.
///
/// # Errors
/// Propagates Kubernetes API and config-compilation failures so the controller's
/// error policy can requeue.
pub async fn reconcile<G: LeaderGuard>(gw: Arc<Gateway>, ctx: Arc<Context<G>>) -> Result<Action> {
    if !ctx.guard.is_leader() {
        // Non-leader: do not mutate cluster or config. Requeue to re-check.
        return Ok(Action::requeue(Duration::from_secs(30)));
    }

    let ns = gw.namespace().unwrap_or_default();
    let api: Api<Gateway> = Api::namespaced(ctx.client.clone(), &ns);

    finalizer(&api, FINALIZER, gw, |event| {
        let ctx = Arc::clone(&ctx);
        async move {
            match event {
                FinalizerEvent::Apply(gw) => apply(&ctx, &gw).await,
                FinalizerEvent::Cleanup(gw) => cleanup(&ctx, &gw).await,
            }
        }
    })
    .await
    .map_err(|e| Error::Finalizer(e.to_string()))
}

/// Finalizer apply step: rebuild the config and mark the gateway accepted.
async fn apply<G: LeaderGuard>(ctx: &Context<G>, gw: &Gateway) -> Result<Action> {
    rebuild(ctx, None).await?;
    mark_accepted(ctx, gw).await?;
    Ok(Action::requeue(REQUEUE))
}

/// Finalizer cleanup step: rebuild the config with the deleted gateway excluded.
async fn cleanup<G: LeaderGuard>(ctx: &Context<G>, gw: &Gateway) -> Result<Action> {
    let ns = gw.namespace().unwrap_or_default();
    rebuild(ctx, Some((&ns, &gw.name_any()))).await?;
    Ok(Action::await_change())
}

/// The error policy: log and requeue with a short backoff.
#[must_use]
pub fn error_policy<G: LeaderGuard>(
    _gw: Arc<Gateway>,
    err: &Error,
    _ctx: Arc<Context<G>>,
) -> Action {
    tracing::warn!(error = %err, "reconcile failed; requeuing");
    Action::requeue(Duration::from_secs(15))
}

/// Run the controller until the stream ends (e.g. on shutdown signal).
///
/// Watches `Gateway` (primary), `HTTPRoute` (mapped to parent gateways via
/// `parentRefs`), and `GatewayClass`. Reconciliation is gated by `ctx.guard`.
///
/// This wiring requires a live cluster and is therefore exercised in production
/// rather than in unit tests; the golden tests cover [`install`] directly.
pub async fn run<G: LeaderGuard>(ctx: Arc<Context<G>>) {
    let gateways: Api<Gateway> = Api::all(ctx.client.clone());
    let routes: Api<HTTPRoute> = Api::all(ctx.client.clone());
    let classes: Api<GatewayClass> = Api::all(ctx.client.clone());

    Controller::new(gateways, watcher::Config::default())
        .watches(routes, watcher::Config::default(), route_to_gateways)
        .watches(
            classes,
            watcher::Config::default(),
            |_class: GatewayClass| {
                // A GatewayClass change affects every gateway of that class, but we
                // cannot enumerate them synchronously here. They are picked up on the
                // next gateway relist; mapping via a reflector store is deferred work.
                std::iter::empty::<ObjectRef<Gateway>>()
            },
        )
        .shutdown_on_signal()
        .run(reconcile, error_policy, ctx)
        .for_each(|res| async move {
            match res {
                Ok((obj, _action)) => tracing::debug!(?obj, "reconciled"),
                Err(err) => tracing::warn!(error = %err, "controller stream error"),
            }
        })
        .await;
}

/// Map an [`HTTPRoute`] to the [`Gateway`]s named in its `parentRefs`, defaulting
/// a missing parent namespace to the route's own.
fn route_to_gateways(route: HTTPRoute) -> impl Iterator<Item = ObjectRef<Gateway>> {
    let route_ns = route.namespace().unwrap_or_default();
    route
        .spec
        .parent_refs
        .into_iter()
        .map(move |p| {
            let ns = p.namespace.unwrap_or_else(|| route_ns.clone());
            ObjectRef::<Gateway>::new(&p.name).within(&ns)
        })
        .collect::<Vec<_>>()
        .into_iter()
}
