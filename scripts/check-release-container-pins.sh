#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# scripts/check-release-container-pins.sh — verify firmware/release-container/
# Dockerfile's apt pins still resolve in the live Ubuntu jammy archive, and
# that its digest-pinned FROM still names a pullable image.
#
# Why this exists: nothing in CI used to build or validate the release
# container. Its libfreetype6-dev pin silently stopped resolving when Ubuntu
# superseded ...1ubuntu0.3 with ...1ubuntu0.4; the first thing that noticed
# was the actual v0.9.0 tag build. This guard makes that failure happen on the
# PR/push that introduces it (or on the next PR after the archive moves),
# with a message that says exactly which pin is stale, what the archive has
# instead, and which recovery procedure to follow.
#
# What it checks:
#   1. Every `FROM` is digest-pinned (`@sha256:<64 hex>`), and that digest is
#      still pullable from Docker Hub.
#   2. Every `pkg=version` in the Dockerfile's `apt-get install` lines uses an
#      `ARG` default (or literal) that resolves to a concrete version, and
#      that exact version is present in at least one of the base image's
#      release/updates/security package indexes (derived from FROM's
#      ubuntu:<tag> — currently jammy for ubuntu:22.04; all four components).
#      The reported "available versions" list is what the archive has now.
#   3. Unpinned apt packages are reported as a WARNING, with one exception:
#      the two `required_pins` (libfreetype6-dev/fonts-dejavu-core) must keep
#      an exact pkg=version pin, so losing either is an ERROR. The Dockerfile
#      pins only those two because their versions can change the firmware
#      image bytes, and its own header explains why pinning every transitive
#      host package would be brittle with no reproducibility gain. Failing
#      every run on the intentionally-unpinned ESP-IDF prerequisites would
#      just force a policy the Dockerfile explicitly rejects. A warning still
#      surfaces a newly-added package for review; a stale, uninstallable, or
#      removed required *pin* is always a hard error.
#
# Usage:
#   scripts/check-release-container-pins.sh                 # default Dockerfile
#   scripts/check-release-container-pins.sh path/Dockerfile
#   scripts/check-release-container-pins.sh --self-test     # offline fixtures
#   scripts/check-release-container-pins.sh --emit-apt-layer # throwaway build
#
# `--emit-apt-layer` prints a minimal Dockerfile containing only the real
# FROM line, ARG defaults, and apt-get install RUN (no toolchain bootstrap),
# so ci.yml's fast gate can build the same apt layer without the slow espup
# install. The self-test and emit modes never touch the network. The self-test
# uses CHECK_RELEASE_PINS_INDEX_FILE to inject a fake package index and
# CHECK_RELEASE_PINS_SKIP_REGISTRY=1 to skip the Docker Hub manifest query;
# both are test seams only, not something CI sets.
#
# Run with bash (the shebang is bash); the repo's scripts are bash-only.
set -euo pipefail

script_path="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
script_dir="$(cd "$(dirname "${script_path}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"
default_dockerfile="${repo_root}/firmware/release-container/Dockerfile"

# Ubuntu release tag -> apt suite. The release container currently builds
# from ubuntu:22.04 (jammy); mapping the tag rather than hardcoding jammy
# means a future base-image bump fails loudly here until this table (and the
# documented recovery procedure) are updated, instead of silently checking
# the new image's pins against the old release's archive. `jammy-backports`
# is intentionally absent from the derived suite list: `apt-get install`
# without `-t jammy-backports` does not select a backports version by
# default, so a pin that only exists there would not actually install. The
# release container builds for linux/amd64 (the same platform release.yml
# runs on), hence binary-amd64 indexes.
declare -A UBUNTU_SUITES=(
  ["20.04"]="focal"
  ["22.04"]="jammy"
  ["24.04"]="noble"
  ["focal"]="focal"
  ["jammy"]="jammy"
  ["noble"]="noble"
)
components=(main restricted universe multiverse)
arch="amd64"
declare -a ACTIVE_SUITES=()
# Packages the Dockerfile's header calls out as byte-reproducibility critical:
# if either loses its exact `pkg=version` pin, the image can still build but
# silently stop being byte-reproducible. That is an ERROR (unlike the
# intentionally-unpinned ESP-IDF prerequisites, which are only warnings).
required_pins=(libfreetype6-dev fonts-dejavu-core)
archive_host="archive.ubuntu.com"
security_host="security.ubuntu.com"
doc_section='docs/release-reproducibility.md, "Rebuilding a release" (the snapshot.ubuntu.com fallback paragraph)'

declare -a TMPDIRS=()
cleanup_tmpdirs() {
  local dir
  for dir in "${TMPDIRS[@]}"; do
    rm -rf "${dir}"
  done
}
trap cleanup_tmpdirs EXIT

FAILURES=0

die() {
  if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
    echo "::error::$*" >&2
  else
    echo "error: $*" >&2
  fi
  exit 1
}

record_error() {
  FAILURES=$((FAILURES + 1))
  if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
    echo "::error::$*" >&2
  else
    echo "error: $*" >&2
  fi
}

warn() {
  if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
    echo "::warning::$*" >&2
  else
    echo "warning: $*" >&2
  fi
}

note() {
  echo "check-release-container-pins: $*"
}

# Joins Dockerfile line continuations and ignores full-line comments. Each
# output line is one complete instruction (RUN ...), with the trailing
# backslash removed. Dockerfile comments cannot appear inside a continued
# instruction, so dropping comment lines unconditionally is safe here.
join_continuations() {
  local file="$1" line buf=""
  while IFS= read -r line || [[ -n "${line}" ]]; do
    line="${line%$'\r'}"
    if [[ "${line}" =~ ^[[:space:]]*# ]]; then
      continue
    fi
    if [[ "${line}" == *\\ ]]; then
      buf+="${line%\\} "
      continue
    fi
    buf+="${line}"
    printf '%s\n' "${buf}"
    buf=""
  done <"${file}"
  if [[ -n "${buf}" ]]; then
    printf '%s\n' "${buf}"
  fi
}

# Expands ${NAME}/$NAME references in an apt version against ARG defaults.
# Fails if a reference has no matching ARG. Bounded to avoid an ARG cycle
# looping forever.
expand_arg_refs() {
  local value="$1" name iterations=0
  while [[ "${value}" =~ \$\{([A-Za-z_][A-Za-z0-9_]*)\} ]]; do
    name="${BASH_REMATCH[1]}"
    if [[ -z "${ARG_VALUES[${name}]+x}" ]]; then
      return 1
    fi
    value="${value//\$\{${name}\}/${ARG_VALUES[${name}]}}"
    iterations=$((iterations + 1))
    if ((iterations > 20)); then
      return 1
    fi
  done
  while [[ "${value}" =~ \$([A-Za-z_][A-Za-z0-9_]*) ]]; do
    name="${BASH_REMATCH[1]}"
    if [[ -z "${ARG_VALUES[${name}]+x}" ]]; then
      return 1
    fi
    value="${value//\$${name}/${ARG_VALUES[${name}]}}"
    iterations=$((iterations + 1))
    if ((iterations > 20)); then
      return 1
    fi
  done
  printf '%s' "${value}"
}

declare -A ARG_VALUES=()
declare -A ARG_DECLARED=()
declare -a ARG_LINES=()
declare -a PINNED=()
declare -a UNPINNED=()
declare -a FROM_REFS=()
declare -a APT_RUN_LINES=()

reset_parse_state() {
  ARG_VALUES=()
  ARG_DECLARED=()
  ARG_LINES=()
  PINNED=()
  UNPINNED=()
  FROM_REFS=()
  APT_RUN_LINES=()
}

parse_args() {
  local file="$1" line line_no=0 spec name value

  while IFS= read -r line || [[ -n "${line}" ]]; do
    line_no=$((line_no + 1))
    line="${line%$'\r'}"
    [[ "${line}" =~ ^[[:space:]]*$ ]] && continue
    [[ "${line}" =~ ^[[:space:]]*# ]] && continue
    if [[ "${line}" =~ ^[[:space:]]*ARG([[:space:]]|$) ]]; then
      spec="${line#"${line%%[![:space:]]*}"}"
      spec="${spec#ARG}"
      spec="${spec#"${spec%%[![:space:]]*}"}"
      spec="${spec%"${spec##*[![:space:]]}"}"
      if [[ -z "${spec}" ]]; then
        die "malformed ARG at ${file}:${line_no}: '${line}' has no name. Expected 'ARG NAME=DEFAULT'."
      fi
      if [[ "${spec}" != *=* ]]; then
        # An ARG with no default is valid Dockerfile (e.g. Docker's own
        # TARGETARCH). Record it so duplicate detection still works, but do
        # not fail unless an apt pin actually references it: expand_arg_refs
        # below reports that case with the pin that needs the default.
        if [[ ! "${spec}" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
          die "malformed ARG at ${file}:${line_no}: '${line}' has an invalid name. Expected 'ARG NAME=DEFAULT' (or a bare 'ARG NAME')."
        fi
        if [[ -n "${ARG_VALUES[${spec}]+x}" || -n "${ARG_DECLARED[${spec}]+x}" ]]; then
          die "duplicate ARG ${spec} at ${file}:${line_no}: already defined earlier. Refusing to guess which default the apt pin consumes."
        fi
        ARG_DECLARED["${spec}"]=1
        ARG_LINES+=("${line}")
        continue
      fi
      name="${spec%%=*}"
      value="${spec#*=}"
      if [[ ! "${name}" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
        die "malformed ARG at ${file}:${line_no}: '${line}' has an invalid name (or spaces around '='). Expected 'ARG NAME=DEFAULT'."
      fi
      if [[ -z "${value}" ]]; then
        die "malformed ARG at ${file}:${line_no}: '${line}' has an empty default. Expected 'ARG ${name}=<version>'."
      fi
      if [[ "${value}" =~ [[:space:]] ]]; then
        die "malformed ARG at ${file}:${line_no}: '${line}' has whitespace in its default. This guard expects pin ARGs to be a single version token."
      fi
      if [[ -n "${ARG_VALUES[${name}]+x}" || -n "${ARG_DECLARED[${name}]+x}" ]]; then
        die "duplicate ARG ${name} at ${file}:${line_no}: already defined earlier. Refusing to guess which default the apt pin consumes."
      fi
      ARG_VALUES["${name}"]="${value}"
      ARG_LINES+=("${line}")
    fi
  done <"${file}"
}

parse_froms() {
  local file="$1" line line_no=0 token image
  local -a tokens=()

  while IFS= read -r line || [[ -n "${line}" ]]; do
    line_no=$((line_no + 1))
    line="${line%$'\r'}"
    [[ "${line}" =~ ^[[:space:]]*# ]] && continue
    if [[ "${line}" =~ ^[[:space:]]*FROM([[:space:]]|$) ]]; then
      # shellcheck disable=SC2206 # intentional word splitting on instruction
      tokens=(${line#*FROM})
      image=""
      for token in "${tokens[@]}"; do
        [[ "${token}" == --* ]] && continue
        image="${token}"
        break
      done
      if [[ -z "${image}" ]]; then
        die "malformed FROM at ${file}:${line_no}: '${line}' names no image."
      fi
      FROM_REFS+=("${image}")
    fi
  done <"${file}"
}

process_package_token() {
  local token="$1" pkg version version_spec arg_name="" prior entry
  local -a previous=()

  if [[ "${token}" != *=* ]]; then
    UNPINNED+=("${token}")
    return
  fi

  pkg="${token%%=*}"
  version_spec="${token#*=}"
  version=""
  if [[ -z "${pkg}" || -z "${version_spec}" ]]; then
    die "malformed apt package pin '${token}': expected pkg=version with both halves non-empty."
  fi
  if ! version="$(expand_arg_refs "${version_spec}")"; then
    die "apt package pin '${token}' references an ARG that is not defined with a default. Add the ARG definition or use a literal version."
  fi
  if [[ -z "${version}" ]]; then
    die "apt package pin '${token}' resolved to an empty version."
  fi
  if [[ "${version_spec}" =~ ^\$\{([A-Za-z_][A-Za-z0-9_]*)\}$ ]]; then
    arg_name="${BASH_REMATCH[1]}"
  elif [[ "${version_spec}" =~ ^\$([A-Za-z_][A-Za-z0-9_]*)$ ]]; then
    arg_name="${BASH_REMATCH[1]}"
  fi

  for prior in "${PINNED[@]}"; do
    IFS='|' read -r -a previous <<<"${prior}"
    if [[ "${previous[0]}" == "${pkg}" ]]; then
      if [[ "${previous[1]}" != "${version}" ]]; then
        die "apt package '${pkg}' is pinned twice with different versions ('${previous[1]}' and '${version}'). Remove one pin."
      fi
      return
    fi
  done
  PINNED+=("${pkg}|${version}|${arg_name}|${token}")
}

package_is_pinned() {
  local pkg="$1" entry
  for entry in "${PINNED[@]}"; do
    if [[ "${entry%%|*}" == "${pkg}" ]]; then
      return 0
    fi
  done
  return 1
}

parse_apt_installs() {
  local file="$1" line segment clean
  local -a tokens=()

  while IFS= read -r line; do
    if [[ "${line}" != *"apt-get"* || "${line}" != *"install"* ]]; then
      continue
    fi
    # Keep the whole joined RUN instruction for --emit-apt-layer. Only emit a
    # line that actually contains an apt-get install invocation.
    if ! printf '%s\n' "${APT_RUN_LINES[@]+"${APT_RUN_LINES[@]}"}" | grep -qxF -- "${line}"; then
      APT_RUN_LINES+=("${line}")
    fi
    while IFS= read -r segment; do
      [[ -n "${segment}" ]] || continue
      clean="${segment//\"/}"
      clean="${clean//\'/}"
      # shellcheck disable=SC2206 # intentional word splitting after quote strip
      tokens=(${clean})
      # tokens[0]=apt-get tokens[1]=install; everything after is package specs
      # and apt flags (flags start with '-' and are skipped below).
      local i token
      for ((i = 2; i < ${#tokens[@]}; i++)); do
        token="${tokens[${i}]}"
        [[ -z "${token}" ]] && continue
        [[ "${token}" == -* ]] && continue
        process_package_token "${token}"
      done
    done < <(grep -oE 'apt-get[[:space:]]+install[^&;|]*' <<<"${line}" || true)
  done < <(join_continuations "${file}")
}

parse_dockerfile() {
  local file="$1"
  [[ -f "${file}" ]] || die "Dockerfile not found: ${file}"
  reset_parse_state
  parse_args "${file}"
  parse_froms "${file}"
  parse_apt_installs "${file}"
}

# Derives the jammy/focal/noble suite pair (release + updates + security) from
# the first FROM's ubuntu:<tag>. Only the first stage matters here: that is
# the stage the apt-get install layer (and --emit-apt-layer) runs in.
resolve_ubuntu_suites() {
  local ref image tag
  ACTIVE_SUITES=()
  if [[ "${#FROM_REFS[@]}" -eq 0 ]]; then
    return
  fi
  ref="${FROM_REFS[0]}"
  image="${ref%@*}"
  tag=""
  if [[ "${image##*/}" == *:* ]]; then
    tag="${image##*:}"
  fi
  if [[ -z "${tag}" ]]; then
    die "cannot derive an Ubuntu apt suite from FROM ${ref}: no ubuntu:<release> tag (e.g. ubuntu:22.04@sha256:...). Add a tag or teach UBUNTU_SUITES in this script about the new image."
  fi
  if [[ -z "${UBUNTU_SUITES[${tag}]+x}" ]]; then
    die "FROM ${ref} uses Ubuntu tag '${tag}', which this script has no apt suite mapping for. Add it to UBUNTU_SUITES (and update the documented recovery procedure) before changing the release container's base image."
  fi
  ACTIVE_SUITES+=("${UBUNTU_SUITES[${tag}]}" "${UBUNTU_SUITES[${tag}]}-updates" "${UBUNTU_SUITES[${tag}]}-security")
}

# Prints "version (suite/component, ...)" for every unique version of pkg in
# the loaded index, or nothing if the package is absent.
available_versions_for() {
  local pkg="$1"
  awk -F '\t' -v pkg="${pkg}" '
    $1 == pkg {
      if (!seen[$2]++) {
        order[++n] = $2
        where[$2] = $3
      } else {
        where[$2] = where[$2] ", " $3
      }
    }
    END {
      for (i = 1; i <= n; i++) {
        printf "%s%s (%s)", (i > 1 ? ", " : ""), order[i], where[order[i]]
      }
      print ""
    }
  ' "${INDEX_FILE}"
}

# Prints the suite/component locations where pkg=version is present.
locations_for() {
  local pkg="$1" version="$2"
  awk -F '\t' -v pkg="${pkg}" -v version="${version}" '
    $1 == pkg && $2 == version {
      if (!seen[$3]++) {
        printf "%s%s", (n++ ? ", " : ""), $3
      }
    }
  ' "${INDEX_FILE}"
}

load_index() {
  if [[ -n "${CHECK_RELEASE_PINS_INDEX_FILE:-}" ]]; then
    [[ -f "${CHECK_RELEASE_PINS_INDEX_FILE}" ]] || die "CHECK_RELEASE_PINS_INDEX_FILE points at a missing file: ${CHECK_RELEASE_PINS_INDEX_FILE}"
    INDEX_FILE="${CHECK_RELEASE_PINS_INDEX_FILE}"
    note "using offline package index ${INDEX_FILE}"
    return
  fi

  local index_dir
  index_dir="$(mktemp -d)"
  TMPDIRS+=("${index_dir}")
  INDEX_FILE="${index_dir}/index.tsv"
  : >"${INDEX_FILE}"

  local -a urls=() pids=() suites_for=() components_for=()
  local suite component host url out i

  for suite in "${ACTIVE_SUITES[@]}"; do
    host="${archive_host}"
    if [[ "${suite}" == *-security ]]; then
      host="${security_host}"
    fi
    for component in "${components[@]}"; do
      url="https://${host}/ubuntu/dists/${suite}/${component}/binary-${arch}/Packages.gz"
      out="${index_dir}/${suite}-${component}.gz"
      (curl -fsSL --retry 3 --retry-delay 2 --max-time 180 "${url}" -o "${out}" &&
        gzip -dc "${out}" >"${out%.gz}") &
      pids+=("$!")
      urls+=("${url}")
      suites_for+=("${suite}")
      components_for+=("${component}")
    done
  done

  for i in "${!pids[@]}"; do
    if ! wait "${pids[${i}]}"; then
      die "could not fetch Ubuntu package index ${urls[${i}]}. This is a network/archive failure, not a pin failure; re-run when the archive is reachable."
    fi
  done

  for i in "${!suites_for[@]}"; do
    suite="${suites_for[${i}]}"
    component="${components_for[${i}]}"
    awk -v suite="${suite}" -v component="${component}" '
      /^Package: / { pkg = substr($0, 10) }
      /^Version: / {
        if (pkg != "") {
          print pkg "\t" substr($0, 10) "\t" suite "/" component
          pkg = ""
        }
      }
    ' "${index_dir}/${suite}-${component}" >>"${INDEX_FILE}"
  done
}

check_base_images() {
  local ref image digest registry repo first token_json token code

  if [[ "${#FROM_REFS[@]}" -eq 0 ]]; then
    record_error "no FROM line found; this guard cannot verify the base image."
    return
  fi

  for ref in "${FROM_REFS[@]}"; do
    if [[ "${ref}" != *"@sha256:"* ]]; then
      record_error "FROM ${ref} is not digest-pinned. Pin it to ubuntu:22.04@sha256:<digest> (see ${doc_section}) so the rootfs cannot drift between builds."
      continue
    fi
    digest="${ref##*@}"
    image="${ref%@*}"
    if [[ ! "${digest}" =~ ^sha256:[0-9a-f]{64}$ ]]; then
      record_error "FROM ${ref} has a malformed digest '${digest}'; expected sha256:<64 lowercase hex characters>."
      continue
    fi
    if [[ "${CHECK_RELEASE_PINS_SKIP_REGISTRY:-0}" == "1" ]]; then
      note "base image ${ref} (registry check skipped by CHECK_RELEASE_PINS_SKIP_REGISTRY=1)"
      continue
    fi

    registry="docker.io"
    repo="${image}"
    if [[ "${repo}" == */* ]]; then
      first="${repo%%/*}"
      if [[ "${first}" == *.* || "${first}" == *:* || "${first}" == "localhost" ]]; then
        registry="${first}"
        repo="${repo#*/}"
      fi
    fi
    if [[ "${registry}" != "docker.io" ]]; then
      record_error "FROM ${ref} uses registry '${registry}', but this guard only knows how to query Docker Hub. Add registry support here or switch the base image back to Docker Hub."
      continue
    fi
    # Strip the tag from the final path component; the registry manifest API
    # wants a repository name plus the digest, never "repo:tag".
    if [[ "${repo##*/}" == *:* ]]; then
      repo="${repo%:*}"
    fi
    if [[ "${repo}" != */* ]]; then
      repo="library/${repo}"
    fi

    if ! token_json="$(curl -fsSL --retry 3 --max-time 60 \
      "https://auth.docker.io/token?service=registry.docker.io&scope=repository:${repo}:pull")"; then
      record_error "could not get a Docker Hub pull token for ${repo}; cannot verify that FROM ${ref} is pullable."
      continue
    fi
    token="$(sed -E 's/.*"token"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/' <<<"${token_json}")"
    if [[ ! "${token}" =~ ^[A-Za-z0-9._-]+$ ]]; then
      record_error "Docker Hub returned an unexpected token response for ${repo}; cannot verify that FROM ${ref} is pullable."
      continue
    fi
    if ! code="$(curl -sS -o /dev/null -w '%{http_code}' --retry 3 --max-time 60 \
      -H "Authorization: Bearer ${token}" \
      -H 'Accept: application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json' \
      "https://registry-1.docker.io/v2/${repo}/manifests/${digest}" 2>/dev/null)"; then
      record_error "could not query Docker Hub for ${ref}; cannot verify the digest is pullable."
      continue
    fi
    if [[ "${code}" != "200" ]]; then
      record_error "FROM ${ref} is not pullable: Docker Hub returned HTTP ${code} for ${repo}@${digest}. Re-resolve the digest and update the Dockerfile (see ${doc_section})."
      continue
    fi
    note "base image ${ref} is pullable"
  done
}

print_unpinned_warning() {
  local pkg list=""
  for pkg in "${UNPINNED[@]}"; do
    list+="${list:+, }${pkg}"
  done
  warn "unpinned apt package(s): ${list}. This is a warning, not an error: the Dockerfile intentionally pins only byte-reproducibility-critical packages (see its header comment). Review any newly-added package here; if its version can change the firmware bytes, pin it. A stale or uninstallable pin is always an error."
}

check() {
  local dockerfile="$1" entry pkg version arg_name spec locations available rel
  rel="${dockerfile#"${repo_root}/"}"

  note "checking ${rel}"
  parse_dockerfile "${dockerfile}"
  resolve_ubuntu_suites

  if [[ "${#FROM_REFS[@]}" -eq 0 ]]; then
    record_error "no FROM line found in ${rel}."
  fi

  load_index

  if [[ "${#PINNED[@]}" -eq 0 ]]; then
    warn "no exact apt pins found in ${rel}; every apt-get install package would be unpinned."
  fi

  local required
  for required in "${required_pins[@]}"; do
    if ! package_is_pinned "${required}"; then
      record_error "required byte-reproducibility package '${required}' has no exact pkg=version pin in ${rel}. The Dockerfile header documents why this package must stay pinned; add '${required}=<version>' (via an ARG if appropriate). If it genuinely no longer participates in byte-reproducibility, update this script's required_pins list and the Dockerfile header together."
    fi
  done

  for entry in "${PINNED[@]}"; do
    IFS='|' read -r pkg version arg_name spec <<<"${entry}"
    locations="$(locations_for "${pkg}" "${version}")"
    if [[ -n "${locations}" ]]; then
      note "OK   ${pkg}=${version} (${locations})"
      continue
    fi
    available="$(available_versions_for "${pkg}")"
    if [[ -z "${available}" ]]; then
      available="none in ${ACTIVE_SUITES[*]} (components: ${components[*]})"
    fi
    record_error "release-container apt pin is stale: ${pkg}=${version} is not available in any ${ACTIVE_SUITES[0]%%.*} suite (${ACTIVE_SUITES[*]}; components: ${components[*]}). Available versions: ${available}. Fix: update ${arg_name:+ARG ${arg_name} in }${rel} to one of the available versions. If the pin must stay exact (e.g. rebuilding an old tag), use the snapshot.ubuntu.com fallback in ${doc_section}."
  done

  if [[ "${#UNPINNED[@]}" -gt 0 ]]; then
    print_unpinned_warning
  fi

  check_base_images

  if ((FAILURES > 0)); then
    note "${FAILURES} problem(s) found"
    exit 1
  fi
  note "all checks passed"
}

emit_apt_layer() {
  local dockerfile="$1" line has_update=0
  parse_dockerfile "${dockerfile}"

  if [[ "${#FROM_REFS[@]}" -eq 0 ]]; then
    die "no FROM line found in ${dockerfile}"
  fi
  if [[ "${#APT_RUN_LINES[@]}" -eq 0 ]]; then
    die "no apt-get install RUN found in ${dockerfile}"
  fi

  echo "# Generated by scripts/check-release-container-pins.sh --emit-apt-layer."
  echo "# Throwaway Dockerfile: the real release container's FROM + ARG defaults"
  echo "# + apt-get install layer only, so CI can validate the apt pins without"
  echo "# bootstrapping ESP-IDF/the esp toolchain. Do not edit or commit."
  echo "FROM ${FROM_REFS[0]}"
  echo 'SHELL ["/bin/bash", "-o", "pipefail", "-c"]'
  echo "ENV DEBIAN_FRONTEND=noninteractive"
  for line in "${ARG_LINES[@]}"; do
    echo "${line}"
  done
  for line in "${APT_RUN_LINES[@]}"; do
    if [[ "${line}" == *"apt-get update"* ]]; then
      has_update=1
    fi
  done
  if ((has_update == 0)); then
    echo "RUN apt-get update"
  fi
  for line in "${APT_RUN_LINES[@]}"; do
    echo "${line}"
  done
}

run_case() {
  local label="$1" dockerfile="$2" expected="$3"
  shift 3
  local -a patterns=("$@")
  local out status pattern
  out="$(mktemp)"
  TMPDIRS+=("${out}")

  set +e
  CHECK_RELEASE_PINS_INDEX_FILE="${SELF_TEST_INDEX}" \
    CHECK_RELEASE_PINS_SKIP_REGISTRY=1 \
    "${script_path}" "${dockerfile}" >"${out}" 2>&1
  status=$?
  set -e

  if [[ "${expected}" == "0" && "${status}" -ne 0 ]]; then
    echo "FAIL: ${label}: expected success, got exit ${status}" >&2
    cat "${out}" >&2
    exit 1
  fi
  if [[ "${expected}" == "nonzero" && "${status}" -eq 0 ]]; then
    echo "FAIL: ${label}: expected failure, got exit 0" >&2
    cat "${out}" >&2
    exit 1
  fi
  for pattern in "${patterns[@]}"; do
    if ! grep -qF -- "${pattern}" "${out}"; then
      echo "FAIL: ${label}: output did not contain '${pattern}'" >&2
      cat "${out}" >&2
      exit 1
    fi
  done
  echo "ok: ${label}"
}

self_test() {
  local tmp good superseded unpinned malformed missing_digest unresolved emitted
  tmp="$(mktemp -d)"
  TMPDIRS+=("${tmp}")
  SELF_TEST_INDEX="${tmp}/index.tsv"

  cat >"${SELF_TEST_INDEX}" <<'EOF'
libfreetype6-dev	1.0.0	jammy-updates/main
libfreetype6-dev	1.0.0	jammy-security/main
libfreetype6-dev	0.4.0	jammy-updates/main
fonts-dejavu-core	2.0.0	jammy/main
EOF

  good="${tmp}/good.Dockerfile"
  cat >"${good}" <<'EOF'
FROM ubuntu:22.04@sha256:0e0a0fc6d18feda9db1590da249ac93e8d5abfea8f4c3c0c849ce512b5ef8982
ARG TARGETARCH
ARG FREETYPE_DEV_VERSION=1.0.0
ARG DEJAVU_CORE_VERSION=2.0.0
RUN apt-get update && apt-get install -y --no-install-recommends \
      "libfreetype6-dev=${FREETYPE_DEV_VERSION}" \
      "fonts-dejavu-core=${DEJAVU_CORE_VERSION}" \
    && rm -rf /var/lib/apt/lists/*
EOF

  superseded="${tmp}/superseded.Dockerfile"
  cat >"${superseded}" <<'EOF'
FROM ubuntu:22.04@sha256:0e0a0fc6d18feda9db1590da249ac93e8d5abfea8f4c3c0c849ce512b5ef8982
ARG FREETYPE_DEV_VERSION=0.3.0
ARG DEJAVU_CORE_VERSION=2.0.0
RUN apt-get update && apt-get install -y --no-install-recommends \
      "libfreetype6-dev=${FREETYPE_DEV_VERSION}" \
      "fonts-dejavu-core=${DEJAVU_CORE_VERSION}" \
    && rm -rf /var/lib/apt/lists/*
EOF

  unpinned="${tmp}/unpinned.Dockerfile"
  cat >"${unpinned}" <<'EOF'
FROM ubuntu:22.04@sha256:0e0a0fc6d18feda9db1590da249ac93e8d5abfea8f4c3c0c849ce512b5ef8982
ARG FREETYPE_DEV_VERSION=1.0.0
ARG DEJAVU_CORE_VERSION=2.0.0
RUN apt-get update && apt-get install -y --no-install-recommends \
      curl \
      "libfreetype6-dev=${FREETYPE_DEV_VERSION}" \
      "fonts-dejavu-core=${DEJAVU_CORE_VERSION}" \
    && rm -rf /var/lib/apt/lists/*
EOF

  malformed="${tmp}/malformed-arg.Dockerfile"
  cat >"${malformed}" <<'EOF'
FROM ubuntu:22.04@sha256:0e0a0fc6d18feda9db1590da249ac93e8d5abfea8f4c3c0c849ce512b5ef8982
ARG FREETYPE_DEV_VERSION = broken
ARG DEJAVU_CORE_VERSION=2.0.0
RUN apt-get update && apt-get install -y --no-install-recommends \
      "libfreetype6-dev=${FREETYPE_DEV_VERSION}" \
      "fonts-dejavu-core=${DEJAVU_CORE_VERSION}"
EOF

  missing_digest="${tmp}/missing-digest.Dockerfile"
  cat >"${missing_digest}" <<'EOF'
FROM ubuntu:22.04
ARG FREETYPE_DEV_VERSION=1.0.0
ARG DEJAVU_CORE_VERSION=2.0.0
RUN apt-get update && apt-get install -y --no-install-recommends \
      "libfreetype6-dev=${FREETYPE_DEV_VERSION}" \
      "fonts-dejavu-core=${DEJAVU_CORE_VERSION}"
EOF

  unpinned_required="${tmp}/unpinned-required.Dockerfile"
  cat >"${unpinned_required}" <<'EOF'
FROM ubuntu:22.04@sha256:0e0a0fc6d18feda9db1590da249ac93e8d5abfea8f4c3c0c849ce512b5ef8982
ARG FREETYPE_DEV_VERSION=1.0.0
RUN apt-get update && apt-get install -y --no-install-recommends \
      "libfreetype6-dev=${FREETYPE_DEV_VERSION}" \
      fonts-dejavu-core
EOF

  unresolved="${tmp}/unresolved-arg.Dockerfile"
  cat >"${unresolved}" <<'EOF'
FROM ubuntu:22.04@sha256:0e0a0fc6d18feda9db1590da249ac93e8d5abfea8f4c3c0c849ce512b5ef8982
ARG FREETYPE_DEV_VERSION=1.0.0
RUN apt-get update && apt-get install -y --no-install-recommends \
      "libfreetype6-dev=${MISSING_VERSION}" \
      "fonts-dejavu-core=2.0.0"
EOF

  run_case "a good Dockerfile passes" "${good}" 0 "libfreetype6-dev=1.0.0" "fonts-dejavu-core=2.0.0" "all checks passed"
  run_case "a superseded pin fails with available versions" "${superseded}" nonzero "libfreetype6-dev" "0.3.0" "0.4.0" "release-reproducibility.md"
  run_case "an unpinned package is reported without failing" "${unpinned}" 0 "unpinned apt package(s)" "curl"
  run_case "removing a required byte-reproducibility pin fails" "${unpinned_required}" nonzero "required byte-reproducibility package" "fonts-dejavu-core"
  run_case "a malformed ARG fails" "${malformed}" nonzero "malformed ARG"
  run_case "a FROM without a digest fails" "${missing_digest}" nonzero "not digest-pinned"
  run_case "an apt pin referencing an undefined ARG fails" "${unresolved}" nonzero "not defined" "MISSING_VERSION"

  emitted="${tmp}/emitted.Dockerfile"
  "${script_path}" --emit-apt-layer "${good}" >"${emitted}"
  for needle in "FROM ubuntu:22.04@sha256:" "ARG FREETYPE_DEV_VERSION=1.0.0" "ARG DEJAVU_CORE_VERSION=2.0.0" "apt-get install"; do
    if ! grep -qF -- "${needle}" "${emitted}"; then
      echo "FAIL: --emit-apt-layer output did not contain '${needle}'" >&2
      cat "${emitted}" >&2
      exit 1
    fi
  done
  if grep -qF -- "espup" "${emitted}"; then
    echo "FAIL: --emit-apt-layer included the esp toolchain bootstrap layer" >&2
    cat "${emitted}" >&2
    exit 1
  fi
  echo "ok: --emit-apt-layer emits only FROM + ARGs + the apt layer"

  echo "check-release-container-pins.sh: all self-test cases passed"
}

main() {
  if [[ "${1:-}" == "--self-test" ]]; then
    self_test
    return
  fi
  if [[ "${1:-}" == "--emit-apt-layer" ]]; then
    emit_apt_layer "${2:-${default_dockerfile}}"
    return
  fi
  check "${1:-${default_dockerfile}}"
}

main "$@"
