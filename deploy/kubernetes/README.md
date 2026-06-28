# Pulsate on Kubernetes

- `deployment.yaml` — Deployment + Service running Pulsate from a mounted ConfigMap.
- `gateway.yaml` — Gateway API `Gateway`/`HTTPRoute` sample (the target the native
  controller reconciles into a `ConfigSnapshot`).

The Gateway API controller, native CRD, EndpointSlice discovery, and a Helm chart
are the next increment (see `docs/16-deployment.md`). Apply the Deployment with a
`pulsate-config` ConfigMap containing your `pulsate.flow`.
