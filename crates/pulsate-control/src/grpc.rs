//! The gRPC admin surface: a `tonic` service mirroring the REST endpoints,
//! plus a server-streaming event feed.
//!
//! Auth and RBAC reuse the same machinery as REST. A bearer-token
//! [`AuthInterceptor`] resolves the token against [`AdminApi`]'s table and
//! stashes the resulting [`Scopes`] in the request extensions; each handler then
//! enforces its required [`Scope`]. Errors carry the same `PLS-ADM-*` codes as
//! the REST surface, surfaced both as the gRPC status code and in a
//! `pls-error-code` status-metadata entry.
//!
//! The `tonic::Status` error type is large, so handlers carry
//! `#[allow(clippy::result_large_err)]` at the module level — matching the same
//! deliberate trade-off made for the REST handlers in `serve.rs`.
#![allow(clippy::result_large_err)]

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use pulsate_core::{Code as PlsCode, Lifecycle};
use tokio::sync::watch;
use tokio_stream::wrappers::{BroadcastStream, TcpListenerStream};
use tokio_stream::{Stream, StreamExt as _};
use tonic::metadata::MetadataValue;
use tonic::service::Interceptor;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

use crate::proto::admin_server::{Admin, AdminServer};
use crate::proto::{
    AuditEntry, AuditLog as ProtoAuditLog, ConfigText, GetAuditLogRequest, GetInfoRequest,
    GetMetricsRequest, Info, ListUpstreamsRequest, Metrics, Problem, ReloadResult, Upstream,
    UpstreamList, ValidateResult, WatchEventsRequest,
};
use crate::{AdminApi, AdminEvent, Scope, Scopes};

/// Build a [`Status`] tagging both the gRPC code and the stable `PLS-ADM-*` code
/// (in the `pls-error-code` status-metadata entry), mirroring the REST problem.
fn status(grpc: tonic::Code, pls: PlsCode, message: &str) -> Status {
    let mut status = Status::new(grpc, message.to_string());
    if let Ok(value) = MetadataValue::try_from(pls.to_string()) {
        status.metadata_mut().insert("pls-error-code", value);
    }
    status
}

/// Missing or unknown bearer token → `Unauthenticated` + `PLS-ADM-0001`.
fn unauthenticated() -> Status {
    status(
        tonic::Code::Unauthenticated,
        PlsCode::ADM_UNAUTHORIZED,
        "missing or unknown bearer token",
    )
}

/// Token lacks the required scope → `PermissionDenied` + `PLS-ADM-0002`.
fn forbidden() -> Status {
    status(
        tonic::Code::PermissionDenied,
        PlsCode::ADM_FORBIDDEN,
        "token lacks the required scope",
    )
}

/// Enforce that the (interceptor-resolved) scopes on `request` satisfy `scope`.
fn require<T>(request: &Request<T>, scope: Scope) -> Result<(), Status> {
    let scopes = request
        .extensions()
        .get::<Scopes>()
        .copied()
        .unwrap_or_default();
    if scopes.satisfies(scope) {
        Ok(())
    } else {
        Err(forbidden())
    }
}

/// Map config diagnostics to proto problems (same shape as the REST surface).
fn problems_of(diags: &[pulsate_config::Diagnostic]) -> Vec<Problem> {
    diags
        .iter()
        .map(|d| {
            let span = d.span();
            Problem {
                code: d.code().to_string(),
                line: span.line,
                col: span.col,
                message: d.message().to_string(),
            }
        })
        .collect()
}

/// Translate a domain [`AdminEvent`] into the wire `Event` oneof.
fn event_to_proto(event: AdminEvent) -> crate::proto::Event {
    use crate::proto::event::Kind;
    let kind = match event {
        AdminEvent::ConfigReloaded { generation } => {
            Kind::ConfigReloaded(crate::proto::ConfigReloaded { generation })
        }
        AdminEvent::LifecycleChanged { state } => {
            Kind::LifecycleChanged(crate::proto::LifecycleChanged { state })
        }
        AdminEvent::AuditAppended { seq, event, hash } => {
            Kind::AuditAppended(crate::proto::AuditAppended { seq, event, hash })
        }
    };
    crate::proto::Event { kind: Some(kind) }
}

/// A bearer-token interceptor reusing the [`AdminApi`] token table. On success
/// it inserts the resolved [`Scopes`] into the request extensions for per-RPC
/// RBAC; on failure it rejects with `Unauthenticated` + `PLS-ADM-0001`.
#[derive(Clone)]
struct AuthInterceptor {
    api: Arc<AdminApi>,
}

impl Interceptor for AuthInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        let token = request
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim);

        let Some(token) = token else {
            return Err(unauthenticated());
        };
        let Some(scopes) = self.api.scopes_for(token) else {
            return Err(unauthenticated());
        };
        request.extensions_mut().insert(scopes);
        Ok(request)
    }
}

/// The tonic service. Every RPC delegates to the shared [`AdminApi`].
struct AdminService {
    api: Arc<AdminApi>,
}

#[tonic::async_trait]
impl Admin for AdminService {
    type WatchEventsStream =
        Pin<Box<dyn Stream<Item = Result<crate::proto::Event, Status>> + Send>>;

    async fn get_info(&self, request: Request<GetInfoRequest>) -> Result<Response<Info>, Status> {
        require(&request, Scope::Read)?;
        Ok(Response::new(Info {
            version: env!("CARGO_PKG_VERSION").to_string(),
            generation: self.api.store.generation(),
            sites: self.api.gateway.router.site_count() as u64,
            upstreams: self.api.gateway.upstreams.len() as u64,
        }))
    }

    async fn get_metrics(
        &self,
        request: Request<GetMetricsRequest>,
    ) -> Result<Response<Metrics>, Status> {
        require(&request, Scope::Read)?;
        Ok(Response::new(Metrics {
            prometheus: self.api.gateway.telemetry.render(),
        }))
    }

    async fn list_upstreams(
        &self,
        request: Request<ListUpstreamsRequest>,
    ) -> Result<Response<UpstreamList>, Status> {
        require(&request, Scope::Read)?;
        let upstreams = self
            .api
            .gateway
            .upstreams
            .summary()
            .into_iter()
            .map(|(name, targets)| Upstream {
                name,
                targets: targets as u64,
            })
            .collect();
        Ok(Response::new(UpstreamList { upstreams }))
    }

    async fn validate_config(
        &self,
        request: Request<ConfigText>,
    ) -> Result<Response<ValidateResult>, Status> {
        require(&request, Scope::Write)?;
        let text = request.into_inner().text;
        let result = match pulsate_config::compile("admin", &text, 0) {
            Ok(compiled) => ValidateResult {
                valid: true,
                problems: problems_of(&compiled.warnings),
            },
            Err(diags) => ValidateResult {
                valid: false,
                problems: problems_of(&diags),
            },
        };
        Ok(Response::new(result))
    }

    async fn reload_config(
        &self,
        request: Request<ConfigText>,
    ) -> Result<Response<ReloadResult>, Status> {
        require(&request, Scope::Write)?;
        let text = request.into_inner().text;
        let result = match self.api.store.reload("admin", &text) {
            Ok(generation) => {
                let generation = generation.snapshot.generation();
                // The reload path pushes a ConfigReloaded event directly.
                self.api.publish(AdminEvent::ConfigReloaded { generation });
                ReloadResult {
                    ok: true,
                    generation,
                    problems: Vec::new(),
                }
            }
            Err(diags) => ReloadResult {
                ok: false,
                generation: 0,
                problems: problems_of(&diags),
            },
        };
        Ok(Response::new(result))
    }

    async fn get_audit_log(
        &self,
        request: Request<GetAuditLogRequest>,
    ) -> Result<Response<ProtoAuditLog>, Status> {
        require(&request, Scope::Admin)?;
        let entries = self
            .api
            .audit
            .entries()
            .into_iter()
            .map(|e| AuditEntry {
                seq: e.seq,
                event: e.event,
                hash: e.hash,
            })
            .collect();
        Ok(Response::new(ProtoAuditLog {
            verified: self.api.audit.verify(),
            entries,
        }))
    }

    async fn watch_events(
        &self,
        request: Request<WatchEventsRequest>,
    ) -> Result<Response<Self::WatchEventsStream>, Status> {
        require(&request, Scope::Admin)?;
        let rx = self.api.subscribe();
        let stream = BroadcastStream::new(rx).filter_map(|res| match res {
            Ok(event) => Some(Ok(event_to_proto(event))),
            // A lagged subscriber drops events rather than tearing down the stream.
            Err(_) => None,
        });
        Ok(Response::new(Box::pin(stream)))
    }
}

/// Pump control-plane events onto the [`AdminApi`] bus until the lifecycle ends.
///
/// `AuditAppended` has no change-notifier, so the log is polled every 250ms and
/// only sequence numbers beyond the start watermark are emitted.
/// `LifecycleChanged` is pushed directly off the watch channel. (`ConfigReloaded`
/// is pushed by the reload RPC itself.)
async fn event_pump(api: Arc<AdminApi>, mut lifecycle: watch::Receiver<Lifecycle>) {
    let mut last_seq = api.audit.entries().last().map(|e| e.seq);
    let mut poll = tokio::time::interval(Duration::from_millis(250));
    loop {
        tokio::select! {
            _ = poll.tick() => {
                for entry in api.audit.entries() {
                    let is_new = last_seq.is_none_or(|seq| entry.seq > seq);
                    if is_new {
                        api.publish(AdminEvent::AuditAppended {
                            seq: entry.seq,
                            event: entry.event.clone(),
                            hash: entry.hash.clone(),
                        });
                        last_seq = Some(entry.seq);
                    }
                }
            }
            changed = lifecycle.changed() => {
                if changed.is_err() {
                    break;
                }
                let state = *lifecycle.borrow();
                api.publish(AdminEvent::LifecycleChanged {
                    state: format!("{state:?}"),
                });
                if state != Lifecycle::Running {
                    break;
                }
            }
        }
    }
}

/// Run the gRPC admin server until the lifecycle leaves `Running`.
///
/// Parallel to [`serve_admin`](crate::serve_admin): it shares one [`AdminApi`]
/// with the REST surface and shuts down gracefully on the same lifecycle signal.
pub async fn serve_grpc(
    listener: tokio::net::TcpListener,
    api: Arc<AdminApi>,
    lifecycle: watch::Receiver<Lifecycle>,
) {
    let pump = tokio::spawn(event_pump(Arc::clone(&api), lifecycle.clone()));

    let service = AdminService {
        api: Arc::clone(&api),
    };
    let interceptor = AuthInterceptor { api };
    let server = AdminServer::with_interceptor(service, interceptor);

    let incoming = TcpListenerStream::new(listener);
    let mut shutdown = lifecycle;
    let _ = Server::builder()
        .add_service(server)
        .serve_with_incoming_shutdown(incoming, async move {
            while *shutdown.borrow_and_update() == Lifecycle::Running {
                if shutdown.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;

    pump.abort();
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use pulsate_config::ConfigStore;
    use pulsate_http::Gateway;
    use pulsate_proxy::Registry;
    use pulsate_router::Router;
    use pulsate_waf::AuditLog;
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;
    use tonic::transport::Channel;

    use super::*;
    use crate::proto::admin_client::AdminClient;

    const CONFIG: &str = "site a.com { route /* ~> respond(status=200) }";

    fn test_api() -> Arc<AdminApi> {
        let store = Arc::new(ConfigStore::load("test", CONFIG).expect("valid test config"));
        let gateway = Arc::new(Gateway::new(
            Arc::new(Router::new(Vec::new())),
            Arc::new(Registry::new()),
        ));
        let audit = Arc::new(AuditLog::new());
        let mut api = AdminApi::new(store, gateway, audit, "admin-secret");
        api.add_token(
            "read-secret",
            Scopes {
                read: true,
                ..Scopes::default()
            },
        );
        Arc::new(api)
    }

    async fn spawn_server(
        api: Arc<AdminApi>,
    ) -> (SocketAddr, watch::Sender<Lifecycle>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = watch::channel(Lifecycle::Running);
        let handle = tokio::spawn(serve_grpc(listener, api, rx));
        // Let tonic begin serving before the client dials.
        tokio::time::sleep(Duration::from_millis(100)).await;
        (addr, tx, handle)
    }

    async fn connect(addr: SocketAddr) -> AdminClient<Channel> {
        AdminClient::connect(format!("http://{addr}"))
            .await
            .expect("connect to grpc server")
    }

    fn bearer<T>(message: T, token: &str) -> Request<T> {
        let mut request = Request::new(message);
        request.metadata_mut().insert(
            "authorization",
            MetadataValue::try_from(format!("Bearer {token}")).unwrap(),
        );
        request
    }

    #[tokio::test]
    async fn get_info_happy_path() {
        let (addr, tx, handle) = spawn_server(test_api()).await;
        let mut client = connect(addr).await;

        let resp = client
            .get_info(bearer(GetInfoRequest {}, "admin-secret"))
            .await
            .expect("get_info succeeds")
            .into_inner();
        assert_eq!(resp.version, env!("CARGO_PKG_VERSION"));

        let _ = tx.send(Lifecycle::Draining);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn audit_log_denied_for_read_only_token() {
        let (addr, tx, handle) = spawn_server(test_api()).await;
        let mut client = connect(addr).await;

        let err = client
            .get_audit_log(bearer(GetAuditLogRequest {}, "read-secret"))
            .await
            .expect_err("read-only token cannot read the audit log");
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
        assert_eq!(
            err.metadata().get("pls-error-code").unwrap(),
            "PLS-ADM-0002"
        );

        let _ = tx.send(Lifecycle::Draining);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn missing_token_is_unauthenticated() {
        let (addr, tx, handle) = spawn_server(test_api()).await;
        let mut client = connect(addr).await;

        let err = client
            .get_info(Request::new(GetInfoRequest {}))
            .await
            .expect_err("a request without a token is rejected");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        assert_eq!(
            err.metadata().get("pls-error-code").unwrap(),
            "PLS-ADM-0001"
        );

        let _ = tx.send(Lifecycle::Draining);
        let _ = handle.await;
    }
}
