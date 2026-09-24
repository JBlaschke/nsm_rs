# Kubernetes manifests

`view-events-rolebinding.yaml` binds a `Role` named `view-events` in the
`nsm-dev` namespace to that namespace's `default` ServiceAccount. The `Role`
itself (`view-events-role.yaml`) and the cluster-scoped variants were treated as
cluster-specific and are git-ignored (see `.gitignore`), so this binding cannot
be applied on its own.

Before reusing it:

- Commit the paired `Role` here so the grant can be reviewed alongside the
  binding, or delete the binding.
- Prefer a dedicated ServiceAccount for the broker pod over `default`, and set
  `automountServiceAccountToken: false` unless the pod needs the API.

Deployment and Service manifests for the broker are a follow-up item in
[the cleanup plan](../../docs/PLAN.md).
