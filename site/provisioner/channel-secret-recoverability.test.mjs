// channel-secret-recoverability.test.mjs — regression coverage for
// meshcadet-provisioner-channel-secret-uncopyable: a generated channel
// secret used to land in `add-channel-secret` (type="password", no reveal,
// no copy button) with nothing but prose telling the operator to "copy it
// somewhere safe" — impossible to act on for a masked field. Because
// del-channel requires the exact secret and the channels table only ever
// shows a 1-byte hash (provisioner.html "Remove channel"), any channel added
// this way was PERMANENTLY unremovable the moment the operator dismissed the
// generated value without having transcribed all 32/64 hex characters by
// eye through the mask (i.e. never).
//
// The fix adds a Show/Hide reveal toggle and a Copy button to
// `add-channel-secret` (mirroring the existing Format B card-URI copy
// affordance and its graceful clipboard-failure fallback — provisioner.js's
// `handleCopyCardUri`), and gates `handleAddChannel` on an explicit
// acknowledgement checkbox whenever the CURRENT field contents were never
// revealed or copied.
//
// Same constraint as every other *.test.mjs in this directory:
// provisioner.js cannot be loaded under plain `node` (top-level
// `document.getElementById` DOM wiring — see guest-password-hygiene.test.mjs's
// header for the full rationale), so this is a static source-text assertion
// test over the shipped provisioner.js/provisioner.html, same pattern as
// channel-secret-hygiene.test.mjs's Part 2.
//
// Plain `node`, zero dependencies (no package.json). Run directly:
//
//   node site/provisioner/channel-secret-recoverability.test.mjs

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

import { extractFunctionBody } from "./secret-hygiene-test-helpers.mjs";

let checks = 0;
function ok(cond, label) {
  assert.ok(cond, label);
  checks++;
}

const here = path.dirname(fileURLToPath(import.meta.url));
const siteDir = path.resolve(here, "..");
const provisionerJs = readFileSync(path.join(siteDir, "provisioner.js"), "utf-8");
const provisionerHtml = readFileSync(path.join(siteDir, "provisioner.html"), "utf-8");

// ── HTML: the reveal toggle, the copy button, and the acknowledgement gate all exist ──

for (const id of ["channel-secret-reveal-button", "channel-secret-copy-button", "channel-secret-ack", "channel-secret-ack-checkbox"]) {
  ok(new RegExp(`id="${id}"`).test(provisionerHtml), `provisioner.html declares #${id}`);
}

// The acknowledgement block starts hidden — it must only appear once
// handleAddChannel decides the current secret is unconfirmed, never by default.
{
  const ackMatch = provisionerHtml.match(/<div id="channel-secret-ack"[^>]*>/);
  ok(ackMatch !== null, "found the channel-secret-ack <div> tag");
  ok(/\bhidden\b/.test(ackMatch[0]), `#channel-secret-ack starts hidden: ${ackMatch[0]}`);
}

// The secret field itself is still type="password" by default in markup —
// the reveal toggle flips this at runtime (JS), it must not be baked into
// the shipped HTML as type="text" (that would defeat the mask entirely for
// every operator, revealed or not, and channel-secret-hygiene.test.mjs
// already pins add-channel-secret's static type="password" separately).
ok(
  /<input id="add-channel-secret"[^>]*type="password"/.test(provisionerHtml),
  "add-channel-secret is still type=\"password\" in the shipped HTML (the reveal toggle flips this at runtime only)"
);

// ── JS: the reveal/copy/confirm plumbing exists and is wired to the right events ──

for (const fn of ["handleToggleChannelSecretVisibility", "handleCopyChannelSecret", "setChannelSecretConfirmed", "resetChannelSecretUi"]) {
  ok(new RegExp(`function\\s+${fn}\\s*\\(`).test(provisionerJs), `provisioner.js defines ${fn}`);
}

ok(
  /channelSecretRevealButton\.addEventListener\("click",\s*handleToggleChannelSecretVisibility\)/.test(provisionerJs),
  "the reveal button's click is wired to handleToggleChannelSecretVisibility"
);
ok(
  /channelSecretCopyButton\.addEventListener\("click",\s*handleCopyChannelSecret\)/.test(provisionerJs),
  "the copy button's click is wired to handleCopyChannelSecret"
);

// A hand-edit of the secret field must invalidate any prior reveal/copy —
// otherwise pasting a NEW secret into an already-confirmed field would
// silently inherit the old confirmation and skip the acknowledgement gate.
ok(
  /addChannelSecret\.addEventListener\("input",\s*\(\)\s*=>\s*setChannelSecretConfirmed\(false\)\)/.test(provisionerJs),
  "typing/pasting into add-channel-secret resets the confirmed flag"
);

// ── Copy handler mirrors handleCopyCardUri's graceful-fallback shape and marks the secret confirmed on success only ──

const handleCopyChannelSecretBody = extractFunctionBody(ok, provisionerJs, "handleCopyChannelSecret");
ok(/navigator\.clipboard\.writeText/.test(handleCopyChannelSecretBody), "handleCopyChannelSecret calls navigator.clipboard.writeText");
ok(/catch/.test(handleCopyChannelSecretBody), "handleCopyChannelSecret has a fallback catch path for a failed clipboard write");
ok(
  /setChannelSecretConfirmed\(true\)/.test(handleCopyChannelSecretBody),
  "a successful copy marks the secret confirmed"
);
// The catch branch must not also confirm — an operator told "copy failed"
// must not be silently exempted from the acknowledgement gate.
{
  const catchIndex = handleCopyChannelSecretBody.indexOf("catch");
  const tryBody = handleCopyChannelSecretBody.slice(0, catchIndex);
  const catchBody = handleCopyChannelSecretBody.slice(catchIndex);
  ok(/setChannelSecretConfirmed\(true\)/.test(tryBody), "confirmation happens in the try (success) branch");
  ok(!/setChannelSecretConfirmed\(true\)/.test(catchBody), "the catch (failure) branch does NOT also confirm the secret");
}

// Revealing (Show) marks confirmed; the toggle function must reference both.
const handleToggleBody = extractFunctionBody(ok, provisionerJs, "handleToggleChannelSecretVisibility");
ok(/setChannelSecretConfirmed\(true\)/.test(handleToggleBody), "revealing the secret (Show) marks it confirmed");

// ── The core invariant: handleAddChannel must not submit an unconfirmed secret without an explicit checkbox ack ──

const handleAddChannelBody = extractFunctionBody(ok, provisionerJs, "handleAddChannel");
ok(/channelSecretConfirmed/.test(handleAddChannelBody), "handleAddChannel consults channelSecretConfirmed");
ok(/channelSecretAckCheckbox\.checked/.test(handleAddChannelBody), "handleAddChannel checks channelSecretAckCheckbox.checked before proceeding when unconfirmed");
ok(/session\.addChannel/.test(handleAddChannelBody), "handleAddChannel still calls session.addChannel on the happy path");
{
  // The addChannel call itself must be reachable only past a `return` that's
  // guarded by the confirmed/checkbox check — i.e. the gate must appear
  // BEFORE the call in source order (a straight-line function, per this
  // file's structure), not merely exist somewhere in the body.
  const gateIndex = handleAddChannelBody.search(/channelSecretConfirmed/);
  const callIndex = handleAddChannelBody.search(/session\.addChannel/);
  ok(gateIndex !== -1 && callIndex !== -1 && gateIndex < callIndex, "the confirmation gate appears before the session.addChannel call");
}

// handleAddChannel resets the reveal/confirm UI on every exit path (success
// AND catch) — same "leave nothing sensitive/stale in the DOM" discipline
// channel-secret-hygiene.test.mjs already pins for the secret VALUE itself.
{
  const clearCount = (handleAddChannelBody.match(/resetChannelSecretUi\(\)/g) || []).length;
  ok(clearCount >= 2, `handleAddChannel calls resetChannelSecretUi() on both the success and catch exit paths (found ${clearCount})`);
}

// clearFormStatuses (the disconnect path) also resets the reveal/confirm UI —
// mirrors channel-secret-hygiene.test.mjs's disconnect-path assertion for
// the secret value itself.
const clearFormStatusesBody = extractFunctionBody(ok, provisionerJs, "clearFormStatuses");
ok(/resetChannelSecretUi\(\)/.test(clearFormStatusesBody), "clearFormStatuses resets the reveal/confirm UI on disconnect");

console.log(`channel-secret-recoverability.test: OK — ${checks} check(s) passed.`);
