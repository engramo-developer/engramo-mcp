# GCP Cloud Run Deployment Guide (HTTP transport)

This deploys `engramo-mcp http` — the Streamable HTTP transport used by ChatGPT and other remote
MCP clients — to Cloud Run, using Cloud Build manual triggers. This mirrors `../engram-api/docs/gcp_deployment_guide.md`
exactly (same project/region, same manual-trigger pattern), simplified because `engramo-mcp` has
**no database and no secrets**: `ENGRAM_API_URL` and `MCP_PUBLIC_URL` are plain per-environment
values (see `src/config.rs`), and `http` mode takes each caller's own bearer token per request
rather than holding a server-side credential. The `stdio` transport (Claude Desktop / Cursor) is
**not** deployed here — see the root `README.md`'s npm install instructions for that.

| Environment | GCP project | Region | Domain (once mapped) |
|---|---|---|---|
| dev | `engram-dev-498121` | `europe-west1` | `mcp-engramo.volmyr.com` |
| prod | `prod-engramo-app` | `europe-west1` | `mcp.engramo.app` |

Both projects already run `engram-api` on this exact project/region pair (`api-engram.volmyr.com`
/ `api.engramo.app`), so no new GCP project or region decision is needed here.

## 0. Verify your target project

Same guard as `engram-api`'s guide — don't trust the current project blindly:

```bash
PROJECT_ID="$(gcloud config get-value project 2>/dev/null)"

if [[ -z "${PROJECT_ID}" ]]; then
    echo "Refusing: no project is set. Run: gcloud config set project <PROJECT_ID>" >&2
    return 1 2>/dev/null || exit 1
fi

STATE=$(gcloud projects describe "${PROJECT_ID}" --format='value(lifecycleState)' 2>&1)
if [[ "${STATE}" != "ACTIVE" ]]; then
    echo "Refusing: project '${PROJECT_ID}' has lifecycleState='${STATE}', not ACTIVE" >&2
    return 1 2>/dev/null || exit 1
fi

echo "Target project: ${PROJECT_ID} (lifecycleState: ${STATE})"
```

Switch with `gcloud config set project engram-dev-498121` (dev) or `gcloud config set project
prod-engramo-app` (prod). Run this whole guide twice, once per project — dev first.

## 1. Grant IAM permissions

No Secret Manager step — there's nothing to store. Just a dedicated build-only service account
(same reasoning as `engram-api`'s guide: never grant deploy powers to the Compute Engine default
SA, since that's the identity the running container itself would authenticate as):

```bash
PROJECT_ID="$(gcloud config get-value project 2>/dev/null)"
[[ -z "${PROJECT_ID}" ]] && { echo "No project set" >&2; return 1 2>/dev/null || exit 1; }

gcloud services enable cloudbuild.googleapis.com run.googleapis.com --project="${PROJECT_ID}"

CLOUD_BUILD_SA="engramo-mcp-cloudbuild@${PROJECT_ID}.iam.gserviceaccount.com"

if ! gcloud iam service-accounts describe "${CLOUD_BUILD_SA}" --project="${PROJECT_ID}" &>/dev/null; then
    gcloud iam service-accounts create engramo-mcp-cloudbuild \
        --project="${PROJECT_ID}" \
        --display-name="Cloud Build (engramo-mcp builds only)"
else
    echo "Service account ${CLOUD_BUILD_SA} already exists, skipping creation."
fi

gcloud projects add-iam-policy-binding "${PROJECT_ID}" \
    --member="serviceAccount:${CLOUD_BUILD_SA}" --role="roles/cloudbuild.builds.builder"
gcloud projects add-iam-policy-binding "${PROJECT_ID}" \
    --member="serviceAccount:${CLOUD_BUILD_SA}" --role="roles/run.admin"
gcloud projects add-iam-policy-binding "${PROJECT_ID}" \
    --member="serviceAccount:${CLOUD_BUILD_SA}" --role="roles/logging.logWriter"
gcloud projects add-iam-policy-binding "${PROJECT_ID}" \
    --member="serviceAccount:${CLOUD_BUILD_SA}" --role="roles/iam.serviceAccountUser"

echo "Cloud Build SA: ${CLOUD_BUILD_SA}"
```

If trigger creation later rejects your user account when picking this SA, grant yourself act-as
rights: `gcloud iam service-accounts add-iam-policy-binding "${CLOUD_BUILD_SA}" --project="${PROJECT_ID}" --member="user:YOUR_EMAIL" --role="roles/iam.serviceAccountUser"`.

## 2. Connect the `engramo-developer` GitHub repo to Cloud Build

**This is the step that differs from `engram-api`'s guide** — `engram-api` still lives under the
`volmyrdot` GitHub account, whose Cloud Build GitHub App connection was set up long ago. `engramo-mcp`
was transferred to the separate `engramo-developer` account, which the Cloud Build GitHub App has
never seen, so it needs its own grant before a trigger can read from it:

1. Go to **Cloud Build → Triggers → Connect Repository** in this project's console (or visit
   `https://github.com/apps/google-cloud-build` directly).
2. Choose **GitHub**, then authenticate/switch to the `engramo-developer` account when prompted
   (not your personal `volmyrdot` account).
3. Grant the app access to the `engramo-developer/engramo-mcp` repository specifically (not "all
   repositories," to keep the grant minimal).
4. Repeat once per project (dev and prod each need their own connection — connections are
   project-scoped, same as `engram-api`'s).

If this repo was already connected while testing something else, skip straight to Section 3.

## 3. Make the service public (one-time, per environment)

Same as `engram-api`'s guide: the very first deploy can fail to set the public-access IAM binding
via `--allow-unauthenticated` (permissions not fully propagated yet). If that happens:

```bash
PROJECT_ID="$(gcloud config get-value project 2>/dev/null)"
gcloud run services add-iam-policy-binding engramo-mcp \
    --project="${PROJECT_ID}" --region=europe-west1 \
    --member=allUsers --role=roles/run.invoker
```

## 4. Review `cloudbuild.yaml`

Repo-root `cloudbuild.yaml` is environment-agnostic via substitutions — no `_APP_ENV` branching
needed since there's no config file per environment, just three plain env vars passed straight to
Cloud Run:

```yaml
substitutions:
  _SERVICE_NAME: 'engramo-mcp'
  _ENGRAM_API_URL: 'https://api-engram.volmyr.com'
  _MCP_PUBLIC_URL: 'https://mcp-engramo.volmyr.com/mcp'
  _ENGRAM_ENABLE_PAID_AI: 'false'
```

The defaults above are **dev** values. The prod trigger (Section 5) overrides `_ENGRAM_API_URL` and
`_MCP_PUBLIC_URL` — leave `_ENGRAM_ENABLE_PAID_AI` as `'false'` on both unless you're deliberately
testing the paid-AI path (see root `README.md`).

`_MCP_PUBLIC_URL` must include the `/mcp` path suffix — it becomes the `resource` value in
`.well-known/oauth-protected-resource` (see `src/well_known.rs`), and ChatGPT resolves the MCP
endpoint from that same path.

## 5. Create the manual trigger

Via console (**Cloud Build → Triggers → Create Trigger**), once per project:

1. Name: `engramo-mcp-dev-manual-deploy` (dev) / `engramo-mcp-prod-manual-deploy` (prod).
2. Region: `europe-west1` (or `global` — either works for the trigger metadata itself).
3. Event: **Manual invocation**.
4. Source: the `engramo-developer/engramo-mcp` repo connected in Section 2, branch `main`.
5. Configuration: **Cloud Build configuration file**, location `cloudbuild.yaml`.
6. Advanced → Service account: `engramo-mcp-cloudbuild@${PROJECT_ID}.iam.gserviceaccount.com` from
   Section 1.
7. Advanced → Substitution variables — **dev**: leave all defaults as-is (they already point at
   dev). **prod**: override
   - `_ENGRAM_API_URL` = `https://api.engramo.app`
   - `_MCP_PUBLIC_URL` = `https://mcp.engramo.app/mcp`
8. Create.

Run it from the CLI once created:

```bash
PROJECT_ID="$(gcloud config get-value project 2>/dev/null)"
gcloud builds triggers run engramo-mcp-dev-manual-deploy --project="${PROJECT_ID}" --branch=main
# or, in the prod project:
gcloud builds triggers run engramo-mcp-prod-manual-deploy --project="${PROJECT_ID}" --branch=main
```

Or just click **Run** on the trigger in the console.

## 6. Map the custom domain (after the first successful deploy)

Cloud Run domain mappings need a **DNS-only (grey-cloud, not proxied)** CNAME in Cloudflare — a
different mechanism from the `wrangler.toml` Worker `custom_domain` routes used by
`study.engramo.app` / `engramo-landing`. Don't reuse that pattern here.

```bash
# dev — run against engram-dev-498121
gcloud beta run domain-mappings create --service=engramo-mcp \
    --domain=mcp-engramo.volmyr.com --region=europe-west1 --project=engram-dev-498121

# prod — run against prod-engramo-app
gcloud beta run domain-mappings create --service=engramo-mcp \
    --domain=mcp.engramo.app --region=europe-west1 --project=prod-engramo-app
```

Each command prints the CNAME target to add. In the **Cloudflare dashboard**, add it as a CNAME
record in the matching zone (`volmyr.com` for dev, `engramo.app` for prod) with the proxy status
set to **DNS only** (grey cloud, not orange). Google's managed TLS cert then provisions
automatically — can take a few minutes up to ~24h.

Verify once DNS + cert are live:

```bash
curl https://mcp-engramo.volmyr.com/.well-known/oauth-protected-resource
```

should return `{"resource":"https://mcp-engramo.volmyr.com/mcp", ...}` — not a placeholder or a
TLS error.

## 7. Verify against the deployed service

Same checks already passed locally this session, now against the real dev URL:

```bash
curl https://mcp-engramo.volmyr.com/.well-known/oauth-protected-resource
curl -X POST https://mcp-engramo.volmyr.com/mcp \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize", ...}'   # expect 401 without a bearer token
```

Then add `https://mcp-engramo.volmyr.com/mcp` as a ChatGPT developer-mode custom connector with an
`engram_` API token minted from **dev** (`https://api-engram.volmyr.com` — Settings → API Tokens on
the dev web app, `study-engramo.volmyr.com`) as the bearer token. Confirm `tools/list` returns 23
tools (paid-AI off) and a real `generate_catalog_with_cards` call succeeds. Only repeat this whole
flow against prod once dev is fully verified end-to-end.
