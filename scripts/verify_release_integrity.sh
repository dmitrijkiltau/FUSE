#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/lib/common.sh"

DIST_DIR="$ROOT/dist"
REPOSITORY=""
WORKFLOW_PATH=".github/workflows/release-artifacts.yml"
WORKFLOW_REF=""
WORKFLOW_NAME=""
EXPECTED_TAG=""
EXPECTED_SHA=""
EXPECTED_REF=""
REQUIRE_SIGNATURES=0
REQUIRE_PROVENANCE=0
SKIP_PACKAGE_VERIFY=0
OIDC_ISSUER="https://token.actions.githubusercontent.com"

usage() {
  cat <<'USAGE'
Usage: scripts/verify_release_integrity.sh [options]

Options:
  --dist <path>            Artifact directory (default: dist)
  --repository <owner/repo>
                           Repository slug used to verify keyless signatures
  --workflow-path <path>   Workflow file path encoded into keyless identity
                           (default: .github/workflows/release-artifacts.yml)
  --workflow-ref <ref>     Workflow git ref used for signing identity/provenance
  --workflow-name <name>   Expected workflow display name in provenance
  --expected-tag <tag>     Expected release tag in provenance
  --expected-sha <sha>     Expected commit SHA in provenance
  --expected-ref <ref>     Expected git ref in provenance (default: --workflow-ref)
  --require-signatures     Require and verify cosign signature sidecars
  --require-provenance     Require release-provenance.json and validate it
  --skip-package-verify    Skip CLI/AOT/VSIX payload structure checks
  --oidc-issuer <url>      Cosign OIDC issuer (default: GitHub Actions)
  -h, --help               Show this help
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dist)
      if [[ -z "${2:-}" ]]; then
        echo "--dist requires a value" >&2
        exit 1
      fi
      DIST_DIR="$(abspath "$2")"
      shift 2
      ;;
    --repository)
      REPOSITORY="${2:-}"
      if [[ -z "$REPOSITORY" ]]; then
        echo "--repository requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --workflow-path)
      WORKFLOW_PATH="${2:-}"
      if [[ -z "$WORKFLOW_PATH" ]]; then
        echo "--workflow-path requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --workflow-ref)
      WORKFLOW_REF="${2:-}"
      if [[ -z "$WORKFLOW_REF" ]]; then
        echo "--workflow-ref requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --workflow-name)
      WORKFLOW_NAME="${2:-}"
      if [[ -z "$WORKFLOW_NAME" ]]; then
        echo "--workflow-name requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --expected-tag)
      EXPECTED_TAG="${2:-}"
      if [[ -z "$EXPECTED_TAG" ]]; then
        echo "--expected-tag requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --expected-sha)
      EXPECTED_SHA="${2:-}"
      if [[ -z "$EXPECTED_SHA" ]]; then
        echo "--expected-sha requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --expected-ref)
      EXPECTED_REF="${2:-}"
      if [[ -z "$EXPECTED_REF" ]]; then
        echo "--expected-ref requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --require-signatures)
      REQUIRE_SIGNATURES=1
      shift
      ;;
    --require-provenance)
      REQUIRE_PROVENANCE=1
      shift
      ;;
    --skip-package-verify)
      SKIP_PACKAGE_VERIFY=1
      shift
      ;;
    --oidc-issuer)
      OIDC_ISSUER="${2:-}"
      if [[ -z "$OIDC_ISSUER" ]]; then
        echo "--oidc-issuer requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ ! -d "$DIST_DIR" ]]; then
  echo "dist directory not found: $DIST_DIR" >&2
  exit 1
fi

if [[ -z "$EXPECTED_REF" ]]; then
  EXPECTED_REF="$WORKFLOW_REF"
fi

payload_names=()
while IFS= read -r name; do
  [[ -n "$name" ]] && payload_names+=("$name")
done < <(list_release_payload_names "$DIST_DIR")

if [[ "${#payload_names[@]}" -eq 0 ]]; then
  echo "no release artifacts found in $DIST_DIR" >&2
  exit 1
fi

sbom_names=()
while IFS= read -r -d '' path; do
  sbom_names+=("$(basename "$path")")
done < <(find "$DIST_DIR" -maxdepth 1 -type f -name '*.spdx.json' -print0)

if [[ "${#sbom_names[@]}" -gt 0 ]]; then
  mapfile -t sbom_names < <(printf '%s\n' "${sbom_names[@]}" | LC_ALL=C sort -u)
fi

if [[ "${#sbom_names[@]}" -ne "${#payload_names[@]}" ]]; then
  echo "expected one SBOM per release payload in $DIST_DIR" >&2
  exit 1
fi

find_payload_for_sbom() {
  local sbom_name="$1"
  local stem="${sbom_name%.spdx.json}"
  local payload
  for payload in "${payload_names[@]}"; do
    if [[ "$(release_artifact_stem "$payload")" == "$stem" ]]; then
      printf '%s\n' "$payload"
      return 0
    fi
  done
  return 1
}

if [[ ! -f "$DIST_DIR/SHA256SUMS" ]]; then
  echo "missing checksums manifest: $DIST_DIR/SHA256SUMS" >&2
  exit 1
fi

checksum_names=()
while IFS= read -r line || [[ -n "$line" ]]; do
  [[ -z "$line" ]] && continue
  expected_hash="${line%%  *}"
  file_name="${line#*  }"
  if [[ ! -f "$DIST_DIR/$file_name" ]]; then
    echo "checksum manifest references missing file: $file_name" >&2
    exit 1
  fi
  actual_hash="$(sha256_for_file "$DIST_DIR/$file_name")"
  if [[ "$actual_hash" != "$expected_hash" ]]; then
    echo "checksum mismatch for $file_name" >&2
    exit 1
  fi
  checksum_names+=("$file_name")
done <"$DIST_DIR/SHA256SUMS"

sorted_payloads="$(printf '%s\n' "${payload_names[@]}")"
sorted_checksums="$(printf '%s\n' "${checksum_names[@]}" | LC_ALL=C sort -u)"
if [[ "$sorted_payloads" != "$sorted_checksums" ]]; then
  echo "SHA256SUMS must cover exactly the release payload archives" >&2
  exit 1
fi

if [[ ! -f "$DIST_DIR/release-artifacts.json" ]]; then
  echo "missing metadata manifest: $DIST_DIR/release-artifacts.json" >&2
  exit 1
fi

export DIST_DIR
export PAYLOADS="$(printf '%s\n' "${payload_names[@]}")"
export SBOMS="$(printf '%s\n' "${sbom_names[@]}")"
export REQUIRE_SIGNATURES
export REQUIRE_PROVENANCE
node <<'NODE'
const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

const dir = process.env.DIST_DIR;
const payloads = process.env.PAYLOADS.split("\n").filter(Boolean);
const sboms = process.env.SBOMS.split("\n").filter(Boolean);
const requireSignatures = process.env.REQUIRE_SIGNATURES === "1";
const requireProvenance = process.env.REQUIRE_PROVENANCE === "1";

function fail(message) {
  throw new Error(message);
}

function sha256(filePath) {
  return crypto.createHash("sha256").update(fs.readFileSync(filePath)).digest("hex");
}

function readJson(fileName) {
  return JSON.parse(fs.readFileSync(path.join(dir, fileName), "utf8"));
}

function listToSet(values) {
  return new Set(values);
}

function expectArrayNames(actualEntries, expectedNames, label) {
  if (!Array.isArray(actualEntries) || actualEntries.length === 0) {
    fail(`${label} array missing or empty`);
  }
  const actualNames = new Set(actualEntries.map((entry) => entry && entry.name));
  const expected = listToSet(expectedNames);
  for (const name of expected) {
    if (!actualNames.has(name)) {
      fail(`${label} missing expected entry: ${name}`);
    }
  }
  if (actualNames.size !== expected.size) {
    fail(`${label} contains unexpected entries`);
  }
}

const metadata = readJson("release-artifacts.json");
expectArrayNames(metadata.artifacts, payloads, "metadata artifacts");

for (const entry of metadata.artifacts) {
  const filePath = path.join(dir, entry.name);
  if (!fs.existsSync(filePath)) {
    fail(`metadata references missing payload: ${entry.name}`);
  }
  if (entry.sha256 !== sha256(filePath)) {
    fail(`metadata sha256 mismatch for ${entry.name}`);
  }
  if (entry.size !== fs.statSync(filePath).size) {
    fail(`metadata size mismatch for ${entry.name}`);
  }
}

const integrity = new Map(
  (metadata.integrityArtifacts || []).map((entry) => [entry.name, entry])
);

const requiredIntegrity = ["SHA256SUMS", ...sboms];
if (requireProvenance) {
  requiredIntegrity.push("release-provenance.json");
}
if (requireSignatures) {
  requiredIntegrity.push("SHA256SUMS.sig", "SHA256SUMS.pem");
  if (requireProvenance) {
    requiredIntegrity.push("release-provenance.sig", "release-provenance.pem");
  }
}

for (const name of requiredIntegrity) {
  const entry = integrity.get(name);
  if (!entry) {
    fail(`metadata missing integrity artifact: ${name}`);
  }
  const filePath = path.join(dir, name);
  if (!fs.existsSync(filePath)) {
    fail(`integrity artifact missing on disk: ${name}`);
  }
  if (entry.sha256 !== sha256(filePath)) {
    fail(`integrity artifact sha256 mismatch for ${name}`);
  }
  if (entry.size !== fs.statSync(filePath).size) {
    fail(`integrity artifact size mismatch for ${name}`);
  }
}

for (const name of sboms) {
  const entry = integrity.get(name);
  const stem = name.replace(/\.spdx\.json$/, "");
  const expectedPayload = payloads.find((payload) =>
    payload.startsWith(`${stem}.`) || payload === stem
  );
  if (!entry || entry.kind !== "sbom") {
    fail(`metadata must classify ${name} as an sbom`);
  }
  if (entry.subject !== expectedPayload) {
    fail(`metadata sbom subject mismatch for ${name}`);
  }
}

if (requireSignatures) {
  const checksumSig = integrity.get("SHA256SUMS.sig");
  const checksumCert = integrity.get("SHA256SUMS.pem");
  if (checksumSig && checksumSig.subject !== "SHA256SUMS") {
    fail("metadata signature subject mismatch for SHA256SUMS.sig");
  }
  if (checksumCert && checksumCert.subject !== "SHA256SUMS") {
    fail("metadata certificate subject mismatch for SHA256SUMS.pem");
  }
  if (requireProvenance) {
    const provSig = integrity.get("release-provenance.sig");
    const provCert = integrity.get("release-provenance.pem");
    if (provSig && provSig.subject !== "release-provenance.json") {
      fail("metadata signature subject mismatch for release-provenance.sig");
    }
    if (provCert && provCert.subject !== "release-provenance.json") {
      fail("metadata certificate subject mismatch for release-provenance.pem");
    }
  }
}
NODE

export EXPECTED_TAG
export EXPECTED_SHA
export EXPECTED_REF
export WORKFLOW_NAME
export WORKFLOW_PATH
node <<'NODE'
const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

const dir = process.env.DIST_DIR;
const payloads = process.env.PAYLOADS.split("\n").filter(Boolean);
const sboms = process.env.SBOMS.split("\n").filter(Boolean);
const expectedTag = process.env.EXPECTED_TAG;
const expectedSha = process.env.EXPECTED_SHA;
const expectedRef = process.env.EXPECTED_REF;
const expectedWorkflowName = process.env.WORKFLOW_NAME;
const expectedWorkflowPath = process.env.WORKFLOW_PATH;
const requireProvenance = process.env.REQUIRE_PROVENANCE === "1";

function fail(message) {
  throw new Error(message);
}

function sha256(filePath) {
  return crypto.createHash("sha256").update(fs.readFileSync(filePath)).digest("hex");
}

function readJson(fileName) {
  return JSON.parse(fs.readFileSync(path.join(dir, fileName), "utf8"));
}

const sbomToPayload = new Map();
for (const sbomName of sboms) {
  const stem = sbomName.replace(/\.spdx\.json$/, "");
  const payloadName = payloads.find((payload) => payload.startsWith(`${stem}.`));
  if (!payloadName) {
    fail(`could not map SBOM to payload: ${sbomName}`);
  }
  sbomToPayload.set(sbomName, payloadName);
}

for (const sbomName of sboms) {
  const doc = readJson(sbomName);
  if (typeof doc.spdxVersion !== "string" || !doc.spdxVersion.startsWith("SPDX-")) {
    fail(`invalid SPDX version in ${sbomName}`);
  }
  if (!Array.isArray(doc.packages) || doc.packages.length !== 1) {
    fail(`expected one package entry in ${sbomName}`);
  }
  if (!Array.isArray(doc.files) || doc.files.length === 0) {
    fail(`SBOM missing file entries: ${sbomName}`);
  }
  if (!Array.isArray(doc.relationships) || doc.relationships.length === 0) {
    fail(`SBOM missing relationships: ${sbomName}`);
  }
  const pkg = doc.packages[0];
  const payloadName = sbomToPayload.get(sbomName);
  if (pkg.packageFileName !== payloadName) {
    fail(`SBOM payload mismatch in ${sbomName}`);
  }
  const checksum = Array.isArray(pkg.checksums)
    ? pkg.checksums.find((entry) => entry.algorithm === "SHA256")
    : undefined;
  if (!checksum) {
    fail(`SBOM missing package SHA256 checksum: ${sbomName}`);
  }
  const actualChecksum = sha256(path.join(dir, payloadName));
  if (checksum.checksumValue !== actualChecksum) {
    fail(`SBOM package checksum mismatch in ${sbomName}`);
  }
  const hasDescribe = doc.relationships.some(
    (entry) =>
      entry.spdxElementId === "SPDXRef-DOCUMENT" &&
      entry.relationshipType === "DESCRIBES" &&
      entry.relatedSpdxElement === "SPDXRef-Package"
  );
  if (!hasDescribe) {
    fail(`SBOM missing DESCRIBES relationship: ${sbomName}`);
  }
}

const provenancePath = path.join(dir, "release-provenance.json");
if (!requireProvenance) {
  process.exit(0);
}

if (!fs.existsSync(provenancePath)) {
  fail("missing release-provenance.json");
}

const provenance = readJson("release-provenance.json");
if (expectedTag && provenance.tag !== expectedTag) {
  fail("provenance tag mismatch");
}
if (expectedSha && provenance.commit !== expectedSha) {
  fail("provenance commit mismatch");
}
if (expectedRef && provenance.ref !== expectedRef) {
  fail("provenance ref mismatch");
}
if (expectedWorkflowName && (!provenance.workflow || provenance.workflow.name !== expectedWorkflowName)) {
  fail("provenance workflow name mismatch");
}
if (!provenance.workflow || provenance.workflow.path !== expectedWorkflowPath) {
  fail("provenance workflow path mismatch");
}
if (expectedRef && provenance.workflow.ref !== expectedRef) {
  fail("provenance workflow ref mismatch");
}
for (const field of ["event", "actor", "runId"]) {
  if (!provenance.workflow[field]) {
    fail(`provenance workflow field missing: ${field}`);
  }
}
if (!Number.isInteger(provenance.workflow.runAttempt) || provenance.workflow.runAttempt < 1) {
  fail("provenance workflow runAttempt must be a positive integer");
}

if (!Array.isArray(provenance.artifacts) || provenance.artifacts.length !== payloads.length) {
  fail("provenance artifacts coverage mismatch");
}
for (const entry of provenance.artifacts) {
  if (!payloads.includes(entry.name)) {
    fail(`unexpected provenance artifact entry: ${entry.name}`);
  }
  const filePath = path.join(dir, entry.name);
  if (entry.sha256 !== sha256(filePath)) {
    fail(`provenance artifact checksum mismatch for ${entry.name}`);
  }
  if (entry.size !== fs.statSync(filePath).size) {
    fail(`provenance artifact size mismatch for ${entry.name}`);
  }
}

if (!Array.isArray(provenance.sboms) || provenance.sboms.length !== sboms.length) {
  fail("provenance sbom coverage mismatch");
}
for (const entry of provenance.sboms) {
  if (!sboms.includes(entry.name)) {
    fail(`unexpected provenance sbom entry: ${entry.name}`);
  }
  if (entry.subject !== sbomToPayload.get(entry.name)) {
    fail(`provenance sbom subject mismatch for ${entry.name}`);
  }
  const filePath = path.join(dir, entry.name);
  if (entry.sha256 !== sha256(filePath)) {
    fail(`provenance sbom checksum mismatch for ${entry.name}`);
  }
  if (entry.size !== fs.statSync(filePath).size) {
    fail(`provenance sbom size mismatch for ${entry.name}`);
  }
}

if (!provenance.checksums || provenance.checksums.name !== "SHA256SUMS") {
  fail("provenance checksums pointer mismatch");
}
if (provenance.checksums.sha256 !== sha256(path.join(dir, "SHA256SUMS"))) {
  fail("provenance SHA256SUMS digest mismatch");
}
if (provenance.checksums.size !== fs.statSync(path.join(dir, "SHA256SUMS")).size) {
  fail("provenance SHA256SUMS size mismatch");
}
NODE

signed_sidecars() {
  local target="$1"
  case "$target" in
    SHA256SUMS)
      printf 'SHA256SUMS.sig\nSHA256SUMS.pem\n'
      ;;
    release-provenance.json)
      printf 'release-provenance.sig\nrelease-provenance.pem\n'
      ;;
    *)
      return 1
      ;;
  esac
}

verify_signed_file() {
  local target="$1"
  local signature certificate
  mapfile -t sidecars < <(signed_sidecars "$target")
  signature="${sidecars[0]}"
  certificate="${sidecars[1]}"

  if [[ ! -f "$DIST_DIR/$signature" ]]; then
    echo "missing signature: $DIST_DIR/$signature" >&2
    exit 1
  fi
  if [[ ! -f "$DIST_DIR/$certificate" ]]; then
    echo "missing certificate: $DIST_DIR/$certificate" >&2
    exit 1
  fi

  cosign verify-blob \
    --certificate "$DIST_DIR/$certificate" \
    --signature "$DIST_DIR/$signature" \
    --certificate-identity "https://github.com/${REPOSITORY}/${WORKFLOW_PATH}@${WORKFLOW_REF}" \
    --certificate-oidc-issuer "$OIDC_ISSUER" \
    "$DIST_DIR/$target" >/dev/null
}

if [[ "$REQUIRE_SIGNATURES" -eq 1 ]]; then
  if [[ -z "$REPOSITORY" || -z "$WORKFLOW_REF" ]]; then
    echo "signature verification requires --repository and --workflow-ref" >&2
    exit 1
  fi
  if ! command -v cosign >/dev/null 2>&1; then
    echo "missing cosign in PATH" >&2
    exit 1
  fi
  verify_signed_file "SHA256SUMS"
  if [[ "$REQUIRE_PROVENANCE" -eq 1 ]]; then
    verify_signed_file "release-provenance.json"
  fi
fi

if [[ "$SKIP_PACKAGE_VERIFY" -eq 0 ]]; then
  for name in "${payload_names[@]}"; do
    case "$name" in
      fuse-cli-*)
        platform="${name#fuse-cli-}"
        platform="${platform%.tar.gz}"
        platform="${platform%.zip}"
        "$ROOT/scripts/verify_cli_artifact.sh" --platform "$platform" --archive "$DIST_DIR/$name"
        ;;
      fuse-aot-*)
        platform="${name#fuse-aot-}"
        platform="${platform%.tar.gz}"
        platform="${platform%.zip}"
        "$ROOT/scripts/verify_aot_artifact.sh" --platform "$platform" --archive "$DIST_DIR/$name"
        ;;
      fuse-vscode-*.vsix)
        platform="${name#fuse-vscode-}"
        platform="${platform%.vsix}"
        "$ROOT/scripts/verify_vscode_vsix.sh" --platform "$platform" --vsix "$DIST_DIR/$name"
        ;;
    esac
  done
fi

echo "release integrity verification passed"
