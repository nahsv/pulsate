//! Minimal Gateway API custom-resource definitions.
//!
//! These are hand-rolled, intentionally-partial mirrors of the upstream Gateway
//! API (<https://gateway-api.sigs.k8s.io>) `v1` types — `GatewayClass`,
//! `Gateway`, and `HTTPRoute` — covering exactly the fields the reconciler reads
//! to build a Pulsate config plus the status sub-resources it writes back.
//!
//! We define them locally via [`kube::CustomResource`] rather than depending on
//! the `gateway-api` crate so the controller pins one `kube`/`k8s-openapi` pair
//! and stays self-contained for cluster-free golden tests. The field names and
//! `#[serde(rename_all = "camelCase")]` shape match the wire format, so swapping
//! in the upstream crate later is a drop-in change.

use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `GatewayClass` — cluster-scoped: declares a controller implementation.
///
/// A `GatewayClass` is "ours" when its [`spec.controllerName`](GatewayClassSpec::controller_name)
/// equals our controller name (see [`crate::CONTROLLER_NAME`]).
#[derive(CustomResource, Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "GatewayClass",
    plural = "gatewayclasses",
    status = "GatewayClassStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct GatewayClassSpec {
    /// The name of the controller that should manage `Gateway`s of this class.
    pub controller_name: String,
}

/// Status of a [`GatewayClass`].
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GatewayClassStatus {
    /// Conditions describing the class's acceptance state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

/// `Gateway` — namespaced: an instance of traffic handling for a class.
#[derive(CustomResource, Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "Gateway",
    plural = "gateways",
    namespaced,
    status = "GatewayStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySpec {
    /// The name of the [`GatewayClass`] this `Gateway` belongs to.
    pub gateway_class_name: String,
    /// The listeners this `Gateway` exposes.
    #[serde(default)]
    pub listeners: Vec<Listener>,
}

/// One listener on a [`Gateway`]: a host/port/protocol triple.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Listener {
    /// A unique (within the `Gateway`) listener name.
    pub name: String,
    /// The hostname this listener serves (may be a wildcard like `*.example.com`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    /// The listening port.
    pub port: u16,
    /// The protocol (`HTTP`, `HTTPS`, …).
    pub protocol: String,
}

/// Status of a [`Gateway`].
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GatewayStatus {
    /// Conditions describing the `Gateway`'s programmed state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

/// `HTTPRoute` — namespaced: HTTP routing rules attached to a parent `Gateway`.
#[derive(CustomResource, Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "HTTPRoute",
    plural = "httproutes",
    namespaced,
    status = "HTTPRouteStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteSpec {
    /// The `Gateway`s (and/or listeners) this route attaches to.
    #[serde(default)]
    pub parent_refs: Vec<ParentRef>,
    /// The hostnames this route answers for (intersected with listener hostnames).
    #[serde(default)]
    pub hostnames: Vec<String>,
    /// The ordered routing rules.
    #[serde(default)]
    pub rules: Vec<HTTPRouteRule>,
}

/// Status of an [`HTTPRoute`].
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteStatus {
    /// Per-parent acceptance conditions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

/// A reference from an [`HTTPRoute`] to a parent [`Gateway`].
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ParentRef {
    /// The parent `Gateway`'s name.
    pub name: String,
    /// The parent's namespace (defaults to the route's own namespace if absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// An optional specific listener (`sectionName`) on the parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_name: Option<String>,
}

/// One routing rule: matches plus the backends to forward to.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteRule {
    /// The match conditions (OR-ed). An empty list matches everything (`/`).
    #[serde(default)]
    pub matches: Vec<HTTPRouteMatch>,
    /// The backend services to forward matched traffic to.
    #[serde(default)]
    pub backend_refs: Vec<HTTPBackendRef>,
}

/// A single match condition on a rule.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HTTPRouteMatch {
    /// The path match (prefix/exact).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<HTTPPathMatch>,
    /// An optional HTTP-method refinement (`GET`, `POST`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

/// A path match on a rule.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HTTPPathMatch {
    /// `PathPrefix` or `Exact` (other types are treated as `PathPrefix`).
    #[serde(rename = "type")]
    pub match_type: String,
    /// The path value (e.g. `/api`).
    pub value: String,
}

impl Default for HTTPPathMatch {
    fn default() -> Self {
        Self {
            match_type: "PathPrefix".to_string(),
            value: "/".to_string(),
        }
    }
}

/// A backend reference: a Kubernetes `Service` and port.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HTTPBackendRef {
    /// The backend `Service` name.
    pub name: String,
    /// The backend namespace (defaults to the route's namespace).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// The backend service port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Relative weight for traffic splitting across backends (defaults to 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
}
