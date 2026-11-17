// Unit tests for src/notify.ts (M3a Task 12b): plain node (`node --test`), TypeScript by type stripping.
// Run from apps/desktop: node --test scripts/notify.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import { Coalescer, COALESCE_MS, DEFAULT_PREFS, shouldNotify } from "../src/notify.ts";

const NOON = new Date(2026, 10, 17, 12, 0, 0);
const ctx = (over = {}) => ({ focused: false, activePeer: null, prefs: { ...DEFAULT_PREFS }, lab: false, now: NOON, ...over });
const msg = (over = {}) => ({
  type: "messageChanged",
  message: {
    id: "m1", peer: "ZS1ABC", direction: "in", body: "hello", state: "received",
    createdUtc: "2026-11-17T10:00:00.000Z", receivedUtc: "2026-11-17T10:00:00.000Z", seq: 1, ...over,
  },
});
const heard = (callsign, source, favourite = false) => ({
  type: "heard",
  station: { callsign, lastHeardUtc: "2026-11-17T10:00:00.000Z", source, grid: null, caps: null, count: 1, favourite, snr: null, bandwidth: null, freq: null, band: null, offsetHz: null },
});

test("focused_active_conversation_does_not_notify", () => {
  assert.equal(shouldNotify(msg(), ctx({ focused: true, activePeer: "ZS1ABC" })), null);
  // Focused on another conversation, or not focused: it notifies.
  assert.ok(shouldNotify(msg(), ctx({ focused: true, activePeer: "ZS2XYZ" })));
  assert.ok(shouldNotify(msg(), ctx({ focused: false, activePeer: "ZS1ABC" })));
});

test("message_in_other_conversation_notifies_with_truncated_plain_text", () => {
  const long = "x".repeat(70) + "\u0007\n\t<b>bold</b> and more words after that";
  const n = shouldNotify(msg({ body: long }), ctx({ focused: true, activePeer: "ZS2XYZ" }));
  assert.equal(n.kind, "message");
  assert.equal(n.title.key, "notify.message.title");
  assert.equal(n.title.params.callsign, "ZS1ABC");
  const text = n.body.params.text;
  assert.ok([...text].length <= 80, `${[...text].length} characters`);
  assert.ok(text.endsWith("…"));
  assert.ok(!/[\u0000-\u001f\u007f]/.test(text), "no control characters");
  assert.ok(text.includes("<b>"), "plain text, kept as text (the OS shows it as such)");
  assert.equal(n.tag, "message:ZS1ABC");
  assert.equal(n.urgent, false);
  // Our own messages and messages still arriving are not "new messages".
  assert.equal(shouldNotify(msg({ direction: "out", state: "queued" }), ctx()), null);
});

test("hide_text_option_omits_the_body", () => {
  const n = shouldNotify(msg(), ctx({ prefs: { ...DEFAULT_PREFS, hideText: true } }));
  assert.equal(n.body, null);
  assert.equal(n.title.params.callsign, "ZS1ABC");
});

test("heard_any_is_off_by_default_and_names_beacon_or_cq", () => {
  assert.equal(shouldNotify(heard("ZS3NEW", "beacon"), ctx()), null);
  const on = ctx({ prefs: { ...DEFAULT_PREFS, heardAny: true } });
  const b = shouldNotify(heard("ZS3NEW", "beacon"), on);
  assert.equal(b.kind, "heardAny");
  assert.deepEqual(b.title, { key: "notify.heard.title", params: { callsign: "ZS3NEW", source: "heard.source.beacon" } });
  assert.equal(shouldNotify(heard("ZS3NEW", "cq"), on).title.params.source, "heard.source.cq");
  // A station heard in a session is not "newly heard on the air".
  assert.equal(shouldNotify(heard("ZS3NEW", "session"), on), null);
  // A favourite notifies by its own switch, on by default.
  assert.equal(shouldNotify(heard("ZS4FAV", "cq", true), ctx()).kind, "heardFavourite");
  // A heard row that only changed (SNR completed, star) is not heard again.
  assert.equal(shouldNotify({ ...heard("ZS3NEW", "cq"), type: "heardUpdated" }, on), null);
});

test("burst_of_heard_is_coalesced", () => {
  const on = ctx({ prefs: { ...DEFAULT_PREFS, heardAny: true } });
  const c = new Coalescer();
  const t0 = 1_000_000;
  const first = c.push(shouldNotify(heard("ZS1AAA", "cq"), on), t0);
  assert.equal(first.title.params.callsign, "ZS1AAA", "the first is shown at once");
  assert.equal(c.push(shouldNotify(heard("ZS2BBB", "beacon"), on), t0 + 2000), null);
  assert.equal(c.push(shouldNotify(heard("ZS3CCC", "cq"), on), t0 + 4000), null);
  assert.deepEqual(c.flush(t0 + 5000), [], "nothing before the window ends");
  const summary = c.flush(t0 + COALESCE_MS);
  assert.equal(summary.length, 1);
  assert.deepEqual(summary[0].title, { key: "notify.heard.more", params: { n: 2 } });
  assert.deepEqual(c.flush(t0 + COALESCE_MS + 1), [], "flushed once");
  // After the window a new one is shown at once again; other kinds are not held by it.
  assert.ok(c.push(shouldNotify(heard("ZS4DDD", "cq"), on), t0 + COALESCE_MS + 10));
  assert.ok(c.push(shouldNotify(msg(), on), t0 + COALESCE_MS + 20));
});

test("lab_profile_defaults_off", () => {
  assert.equal(shouldNotify(msg(), ctx({ lab: true })), null);
  assert.equal(shouldNotify({ type: "modemState", state: { type: "faulted" }, detail: "x" }, ctx({ lab: true })), null);
  assert.ok(shouldNotify(msg(), ctx({ lab: true, prefs: { ...DEFAULT_PREFS, labEnabled: true } })));
  // The master switch turns everything off.
  assert.equal(shouldNotify(msg(), ctx({ prefs: { ...DEFAULT_PREFS, enabled: false } })), null);
});

test("quiet_hours_suppress_all_but_urgent_faults", () => {
  const quiet = { ...DEFAULT_PREFS, quietStart: "22:00", quietEnd: "06:30" };
  const night = new Date(2026, 10, 17, 23, 15);
  const early = new Date(2026, 10, 18, 6, 29);
  const morning = new Date(2026, 10, 18, 6, 30);
  for (const now of [night, early]) {
    assert.equal(shouldNotify(msg(), ctx({ prefs: quiet, now })), null, now.toString());
    assert.equal(shouldNotify({ type: "callFailed", peer: "ZS1ABC", reason: "no answer" }, ctx({ prefs: quiet, now })), null);
    const fault = shouldNotify({ type: "modemState", state: { type: "faulted" }, detail: "control port closed" }, ctx({ prefs: quiet, now }));
    assert.equal(fault.kind, "fault");
    assert.equal(fault.urgent, true);
  }
  assert.ok(shouldNotify(msg(), ctx({ prefs: quiet, now: morning })), "quiet hours end at 06:30");
  // A same-day window (not across midnight) works too.
  const lunch = { ...DEFAULT_PREFS, quietStart: "12:00", quietEnd: "13:00" };
  assert.equal(shouldNotify(msg(), ctx({ prefs: lunch, now: NOON })), null);
});

test("every kind is behind its own switch", () => {
  const off = { ...DEFAULT_PREFS, incoming: false, callFailed: false, delivery: false, fault: false };
  const session = { type: "session", session: { peer: "ZS1ABC", direction: "incoming", sinceUtc: "2026-11-17T10:00:00.000Z", peerApp: null, sessionId: "s", bandwidth: "2300" } };
  assert.equal(shouldNotify(session, ctx()).kind, "incoming");
  assert.equal(shouldNotify({ ...session, session: { ...session.session, direction: "outgoing" } }, ctx()), null, "our own call is not news");
  assert.equal(shouldNotify(session, ctx({ prefs: off })), null);
  assert.equal(shouldNotify(msg({ direction: "out", state: "uncertain" }), ctx()).kind, "delivery");
  assert.equal(shouldNotify(msg({ direction: "out", state: "failed" }), ctx({ prefs: off })), null);
  const refused = { type: "broadcast", kind: "beacon", outcome: { type: "refused", reason: "not sent: x" } };
  assert.equal(shouldNotify(refused, ctx()), null, "beaconRefused is off by default");
  assert.equal(shouldNotify(refused, ctx({ prefs: { ...DEFAULT_PREFS, beaconRefused: true } })).kind, "beaconRefused");
});
