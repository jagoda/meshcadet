// flash.js — version selector for the web flasher (site/flash.html).
//
// Two data sources, deliberately different origins:
//
//   1. The release LIST comes from a live client-side fetch of the GitHub
//      REST API (api.github.com), which sends `Access-Control-Allow-Origin:
//      *` and so is fetchable cross-origin from a Pages page. This is what
//      keeps the dropdown always up to date with no redeploy needed.
//
//   2. The actual flashable MANIFEST/BINARY is never fetched from GitHub
//      Releases directly — verified live (curl against a real GitHub
//      release asset) that the release-download redirect chain
//      (github.com -> release-assets.githubusercontent.com) carries no
//      Access-Control-Allow-Origin header at all, so browser fetch() of a
//      Release asset from this Pages origin is CORS-blocked. Instead,
//      .github/workflows/pages-deploy.yml mirrors each recent release's
//      manifest.json + merged .bin into site/firmware/<tag>/ at deploy
//      time, and esp-web-install-button is pointed at that same-origin
//      copy. That mirror step is the reason a just-published release can
//      briefly appear in this dropdown before its assets exist on Pages —
//      in that window esp-web-install-button's own connect-time error
//      dialog surfaces the 404 (not handled specially here; see
//      docs/adr/0006-web-flasher.md Consequences for why that's accepted).
//
// No build step (site/README.md convention) — this is plain ES module JS
// loaded directly by the browser, no bundler.

const REPO = "jagoda/meshcadet";
const RELEASES_API = `https://api.github.com/repos/${REPO}/releases`;

// Keep this equal to the mirror cap in .github/workflows/pages-deploy.yml's
// "Mirror recent release firmware assets" step — there is no shared build
// step to enforce that automatically, so a version beyond this count is
// guaranteed to exist on GitHub but NOT guaranteed to be mirrored onto
// Pages. If you change one, change the other.
const MAX_VERSIONS = 8;

// Only tags release.yml actually builds (v*.*.* triggers it) are real
// firmware releases; anything else on the releases list (there shouldn't
// be anything else, but defense in depth) is filtered out.
const VERSION_TAG_RE = /^v\d+\.\d+\.\d+$/;

const select = document.getElementById("version-select");
const status = document.getElementById("version-status");
const installButton = document.getElementById("install-button");

function manifestUrlFor(tag) {
  // Relative, no leading slash — this is a project Pages site
  // (/meshcadet/...), see site/README.md "Conventions".
  return `firmware/${tag}/manifest.json`;
}

function setStatus(message) {
  status.textContent = message;
}

function selectVersion(tag) {
  const url = manifestUrlFor(tag);
  installButton.manifest = url;
  installButton.setAttribute("manifest", url);
  installButton.hidden = false;
}

function renderEmpty(message) {
  select.innerHTML = "";
  const option = document.createElement("option");
  option.textContent = message;
  select.append(option);
  select.disabled = true;
  installButton.hidden = true;
}

function renderReleases(releases) {
  if (releases.length === 0) {
    renderEmpty("No published releases yet");
    setStatus(
      "MeshCadet hasn't cut a tagged release yet — check back soon, or " +
        "build from source (see the project README)."
    );
    return;
  }

  select.innerHTML = "";
  for (const release of releases) {
    const option = document.createElement("option");
    option.value = release.tag_name;
    option.textContent = release.name?.trim() || release.tag_name;
    select.append(option);
  }
  select.disabled = false;
  select.value = releases[0].tag_name;
  selectVersion(releases[0].tag_name);
  setStatus(`Showing the ${releases.length} most recent release${releases.length === 1 ? "" : "s"}.`);

  select.addEventListener("change", () => {
    selectVersion(select.value);
  });
}

async function loadReleases() {
  let response;
  try {
    response = await fetch(RELEASES_API, {
      headers: { Accept: "application/vnd.github+json" },
    });
  } catch (err) {
    // Logged for anyone debugging via devtools; the on-page status message
    // below is the actionable, non-technical version for everyone else.
    console.error("MeshCadet flasher: failed to reach the GitHub releases API", err);
    renderEmpty("Couldn't reach GitHub");
    setStatus(
      "Couldn't reach the GitHub API to list releases (offline, or a " +
        "network/CORS issue) — reload to retry, or get firmware directly " +
        "from the GitHub Releases page linked below."
    );
    return;
  }

  if (!response.ok) {
    console.error(
      `MeshCadet flasher: GitHub releases API returned ${response.status} ${response.statusText}`
    );
    renderEmpty("Couldn't load releases");
    setStatus(
      response.status === 403
        ? "GitHub API rate limit hit — reload in a bit, or get firmware " +
            "directly from the GitHub Releases page linked below."
        : `GitHub API returned ${response.status} — reload to retry, or ` +
            "get firmware directly from the GitHub Releases page linked below."
    );
    return;
  }

  const releases = (await response.json())
    .filter((r) => !r.draft && !r.prerelease && VERSION_TAG_RE.test(r.tag_name))
    .sort((a, b) => new Date(b.published_at) - new Date(a.published_at))
    .slice(0, MAX_VERSIONS);

  renderReleases(releases);
}

loadReleases();
