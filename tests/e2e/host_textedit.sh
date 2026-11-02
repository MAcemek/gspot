#!/usr/bin/env bash
# G1 + G2 gate: the app reads a real TextEdit document, lints it, draws
# geometry, applies a suggestion into TextEdit, and TextEdit shows the change.
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"
FIXTURE="I has teh cat and I think that we should just ship it. Teh end. In order to be able to make progress at this point in time, it was decided by me that we should send a very large number of emails to the founders. Because the venue had to be booked six months ahead and the caterer needed a final headcount and the speakers could only confirm after their visa interviews, but the sponsors still wanted a date before the end of the quarter so the date moved to May 14."
# `every document whose text starts with` errors out in TextEdit (-1700); walk the indexes backwards instead.
close_fixture_docs() {
  osascript >/dev/null 2>&1 <<'APPLESCRIPT'
tell application "TextEdit"
  set n to count documents
  repeat with i from n to 1 by -1
    try
      if (text of document i) starts with "I has" then close document i saving no
    end try
  end repeat
end tell
APPLESCRIPT
}
cleanup() { close_fixture_docs; stop_app; }
trap cleanup EXIT

# Earlier runs dismiss a delivery alert and that persists; reset it so the fixture is deterministic.
SETTINGS="$HOME/Library/Application Support/com.publik.grammarlama/settings.json"
[ -f "$SETTINGS" ] && python3 -I - "$SETTINGS" <<'PY'
import json, sys
p = sys.argv[1]
try:
    d = json.load(open(p))
except Exception:
    sys.exit(0)
d["dismissed"] = []
json.dump(d, open(p, "w"))
PY
wait_idle "${IDLE_S:-6}" "${IDLE_MAX_S:-180}" || { echo "SKIPPED (call in progress)"; exit 2; }
start_app
H="$(curl -s "http://127.0.0.1:$PORT/health")"
if [ "$(echo "$H" | jsonq 'd["trusted"]')" = "True" ]; then pass "process is trusted for Accessibility"; else fail "not trusted for Accessibility (grant it to the launcher or the app)"; summary; exit 1; fi
if wait_for 'set(d["ui_ready"]) >= {"panel","gbutton","underlines","tab"}' 15; then pass "panel, G button, underline and edge-tab webviews mounted"; else fail "webviews not mounted: $(probe /state | jsonq 'd["ui_ready"]') log=$(probe /state | jsonq 'd["ui_log"]')"; fi

# Tuck: the app starts with the panel slid away behind the edge tab on the dock side.
S="$(probe /state)"
[ "$(echo "$S" | jsonq 'd["panel"]["tucked"] and d["panel"]["tab_visible"] and not d["panel"]["visible"]')" = "True" ] && pass "starts tucked: edge tab shown, panel hidden" || fail "tuck state at start: $(echo "$S" | jsonq 'd["panel"]')"
probe /panel '{"tucked": false}' >/dev/null
if wait_for 'd["panel"]["visible"] and not d["panel"]["tab_visible"] and not d["panel"]["tucked"] and abs(d["panel"]["x"] - 12) < 1' 5; then pass "untuck slides the panel out to the left dock (x=12)"; else fail "untuck: $(probe /state | jsonq 'd["panel"]')"; fi
probe /panel '{"tucked": true}' >/dev/null
if wait_for 'd["panel"]["tucked"] and not d["panel"]["visible"] and d["panel"]["tab_visible"]' 5; then pass "tuck slides the panel away and shows the edge tab"; else fail "tuck: $(probe /state | jsonq 'd["panel"]')"; fi

osascript >/dev/null <<APPLESCRIPT
tell application "TextEdit"
  activate
  set d to make new document
  set text of d to "$FIXTURE"
end tell
APPLESCRIPT
sleep 0.6
# Put the caret in the text area so the field is focused.
osascript -e 'tell application "System Events" to tell process "TextEdit" to keystroke (ASCII character 31)' >/dev/null 2>&1 || true

if wait_for 'd["doc"] is not None and d["doc"]["text"].startswith("I has teh cat")' 8; then pass "focused TextEdit document is read through AX"; else fail "document not read: $(probe /state | head -c 300)"; summary; exit 1; fi
# The 47-word fourth sentence: Harper's note carries a lossless split at once, before any model runs.
if wait_for 'any(a["kind"]=="Readability" and a["suggestions"] and ". But the sponsors" in a["suggestions"][0] for a in d["alerts"])' 10; then pass "long-sentence note carries an instant split (Accept before the spot check)"; else fail "long-sentence note without a split: $(probe /state | jsonq '[(a["kind"], a["suggestions"][:1]) for a in d["alerts"] if a["kind"]=="Readability"]')"; fi
S="$(probe /state)"
echo "$S" | jsonq 'd["doc"]["app_name"]' | grep -q TextEdit && pass "app name is TextEdit" || fail "app name: $(echo "$S" | jsonq 'd["doc"]["app_name"]')"
[ "$(echo "$S" | jsonq 'd["doc"]["frame"]["w"] > 100 and d["doc"]["frame"]["h"] > 50')" = "True" ] && pass "element frame has a size" || fail "frame: $(echo "$S" | jsonq 'd["doc"]["frame"]')"

if wait_for 'any(a["original"]=="teh" for a in d["alerts"])' 6; then pass "Harper flags 'teh'"; else fail "no 'teh' alert: $(probe /state | jsonq '[a["original"] for a in d["alerts"]]')"; fi
S="$(probe /state)"
[ "$(echo "$S" | jsonq 'any(a["category"]=="delivery" for a in d["alerts"])')" = "True" ] && pass "delivery rule fires on hedging" || fail "no delivery alert"
[ "$(echo "$S" | jsonq 'd["score"]["words"] >= 10')" = "True" ] && pass "score computed ($(echo "$S" | jsonq 'd["score"]["words"]') words)" || fail "score missing"
# Geometry is recomputed every tick; wait until the 'teh' underline has a rect (the window is still settling right after activation).
TEH="$(echo "$S" | jsonq 'next((a["id"] for a in d["alerts"] if a["original"]=="teh"), "")')"
if wait_for "any(r['id']=='$TEH' for r in d['geometry']['rects'])" 8; then pass "underline geometry computed ($(probe /state | jsonq 'len(d["geometry"]["rects"])') rects)"; else fail "no underline rect for 'teh': $(probe /state | jsonq 'len(d["geometry"]["rects"])') rects"; fi
G="$(probe /state | jsonq 'len(d["geometry"]["rects"])')"

# Hover: a synthetic pointer move over the 'teh' underline opens the card; moving away closes it
# (regression: the card used to stay open because the overlay never received DOM mouseleave).
M0="$(probe /mouse)"
S="$(probe /state)"
AID="$(echo "$S" | jsonq 'next((a["id"] for a in d["alerts"] if a["original"]=="teh"), "")')"
HX="$(echo "$S" | jsonq "next((round(d['geometry']['origin_x']+r['x']+r['w']/2, 1) for r in d['geometry']['rects'] if r['id']=='$AID'), None)")"
HY="$(echo "$S" | jsonq "next((round(d['geometry']['origin_y']+r['y']+r['h']/2, 1) for r in d['geometry']['rects'] if r['id']=='$AID'), None)")"
# A synthetic move is overridden the moment the physical mouse moves; verify it landed (3 tries) or skip the hover checks.
move_pointer() { # move_pointer x y -> 0 when the pointer is there afterwards
  local x="$1" y="$2" i
  for i in 1 2 3; do
    probe /mouse "{\"x\": $x, \"y\": $y}" >/dev/null; sleep 0.3
    if [ "$(probe /mouse | jsonq "abs(d['x'] - ($x)) < 3 and abs(d['y'] - ($y)) < 3")" = "True" ]; then return 0; fi
    sleep 0.5
  done
  return 1
}
if [ -n "$AID" ] && [ "$HX" != "None" ] && ! move_pointer "$HX" "$HY"; then
  echo "  ----  hover checks skipped: the pointer is being moved by someone (now at $(probe /mouse)); rerun when idle"
elif [ -n "$AID" ] && [ "$HX" != "None" ]; then
  if wait_for "d['geometry']['hovered']=='$AID'" 3; then pass "pointer over the 'teh' underline is detected as hover"; else fail "hover not detected: $(probe /state | jsonq 'd["geometry"]["hovered"]')"; fi
  if wait_for 'len(d["card_rects"]) > 0' 3; then pass "hover card opens"; else fail "hover card did not open"; fi
  # Walk the pointer onto the card itself (the path that used to leave the card stuck open).
  S2="$(probe /state)"
  CX="$(echo "$S2" | jsonq 'round(d["card_rects"][0]["x"] + d["card_rects"][0]["w"]/2, 1)')"
  CY="$(echo "$S2" | jsonq 'round(d["card_rects"][0]["y"] + d["card_rects"][0]["h"]/2, 1)')"
  move_pointer "$CX" "$CY" || echo "  ----  pointer did not settle on the card (someone moved the mouse?)"
  if wait_for 'd["geometry"]["over_card"] and len(d["card_rects"]) > 0' 3; then pass "pointer over the card keeps it open"; else fail "card did not stay open under the pointer: $(probe /state | jsonq 'd["geometry"]["over_card"], len(d["card_rects"])')"; fi
  AX="$(echo "$S" | jsonq 'round(d["geometry"]["origin_x"] + d["geometry"]["width"]/2, 1)')"
  AY="$(echo "$S" | jsonq 'round(max(d["geometry"]["origin_y"] - 30, 30), 1)')"
  move_pointer "$AX" "$AY" || echo "  ----  pointer did not settle away from the field (someone moved the mouse?)"
  if wait_for 'd["geometry"]["hovered"] is None and not d["geometry"]["over_card"]' 3; then pass "pointer away: hover cleared"; else fail "hover still set: $(probe /state | jsonq 'd["geometry"]["hovered"], d["geometry"]["over_card"]')"; fi
  if wait_for 'len(d["card_rects"]) == 0' 3; then pass "hover card closes after the pointer leaves"; else fail "hover card stuck open: $(probe /state | jsonq 'd["card_rects"]')"; fi
  probe /mouse "{\"x\": $(echo "$M0" | jsonq 'd["x"]'), \"y\": $(echo "$M0" | jsonq 'd["y"]')}" >/dev/null
else
  fail "no underline rect for 'teh' to hover (geometry rects: $G)"
fi

ID="$(echo "$S" | jsonq 'next(a["id"] for a in d["alerts"] if a["original"]=="teh")')"
R="$(probe /accept "{\"id\": \"$ID\", \"suggestion\": 0}" 30)"
if echo "$R" | grep -q '"method"'; then pass "accept applied via $(echo "$R" | jsonq 'd["method"]')"; else fail "accept failed: $R"; fi
sleep 0.5
TXT="$(osascript -e 'tell application "TextEdit" to get text of document 1')"
case "$TXT" in
  "I has the cat"*) pass "TextEdit now reads: ${TXT:0:40}…" ;;
  *) fail "TextEdit text is: $TXT" ;;
esac
if wait_for 'not any(a["original"]=="teh" and a["start"] < 12 for a in d["alerts"])' 5; then pass "alert list re-linted after the edit"; else fail "stale alert remains"; fi

# dismiss persists
ID2="$(probe /state | jsonq 'next((a["id"] for a in d["alerts"] if a["category"]=="delivery"), "")')"
if [ -n "$ID2" ]; then
  probe /dismiss "{\"id\": \"$ID2\"}" >/dev/null
  if wait_for "not any(a['id']=='$ID2' for a in d['alerts'])" 3; then pass "dismiss removes the alert"; else fail "dismiss did not remove"; fi
fi

# Spot check on the live document: the wordy third sentence gets a sentence-level
# Clarity suggestion anchored to the real text, and accepting it rewrites TextEdit.
SP="$(probe /feature '{"name":"spot_check","args":{}}' 300)"
if [ "$(echo "$SP" | jsonq 'd.get("suggestions", 0) >= 1' 2>/dev/null)" = "True" ]; then
  pass "spot check on the live document: $(echo "$SP" | jsonq 'd["suggestions"]') suggestion(s) from $(echo "$SP" | jsonq 'd["checked"]') sentences in $(echo "$SP" | jsonq 'd["ms"]') ms"
  S="$(probe /state)"
  SID="$(echo "$S" | jsonq 'next((a["id"] for a in d["alerts"] if a["kind"].startswith("Spot") and "In order to" in a["original"]), "")')"
  if [ -n "$SID" ]; then
    pass "the wordy sentence is flagged ($(echo "$S" | jsonq "next(a['title'] for a in d['alerts'] if a['id']=='$SID')"))"
    [ "$(echo "$S" | jsonq "next(a['category']=='clarity' and d['doc']['text'].encode('utf-16-le')[a['u16_start']*2:a['u16_end']*2].decode('utf-16-le')==a['original'] for a in d['alerts'] if a['id']=='$SID')")" = "True" ] && pass "spot alert is Clarity and anchored to the document text" || fail "spot alert anchoring"
    if wait_for "any(r['id']=='$SID' for r in d['geometry']['rects'])" 8; then pass "sentence underline drawn in TextEdit ($(probe /state | jsonq "sum(1 for r in d['geometry']['rects'] if r['id']=='$SID')") line rect(s))"; else fail "no sentence underline rect: active=$(probe /state | jsonq 'd["doc"]["active"]') rects=$(probe /state | jsonq 'len(d["geometry"]["rects"])') range=$(probe /state | jsonq "next((a['u16_start'], a['u16_end']) for a in d['alerts'] if a['id']=='$SID')") frame=$(probe /state | jsonq 'd["doc"]["visible"]')"; fi
    SUG="$(echo "$S" | jsonq "next(a['suggestions'][0] for a in d['alerts'] if a['id']=='$SID')")"
    R="$(probe /accept "{\"id\": \"$SID\", \"suggestion\": 0}" 30)"
    sleep 0.6
    TXT2="$(osascript -e 'tell application "TextEdit" to get text of document 1')"
    if echo "$R" | grep -q '"method"' && ! echo "$TXT2" | grep -q "In order to be able"; then pass "accepting the rewrite changed TextEdit (now: …${TXT2:60:70}…)"; else fail "accept spot: $R / text: $TXT2"; fi
    if wait_for "not any(a['id']=='$SID' for a in d['alerts'])" 5; then pass "spot alert gone after accept"; else fail "spot alert remains"; fi
  else
    fail "no spot alert for the wordy sentence: $(echo "$S" | jsonq '[(a["kind"], a["original"][:30]) for a in d["alerts"] if a["kind"].startswith("Spot")]')"
  fi
else
  fail "spot check: $(echo "$SP" | head -c 240)"
fi
# Terminals show a command transcript, not writing: Terminal in front must attach nothing.
if pgrep -xq Terminal; then
  osascript -e 'tell application "Terminal" to activate' >/dev/null 2>&1 || true
  if wait_for 'd["status"]["skipped"] and d["status"]["frontmost_bid"] == "com.apple.Terminal" and (d["doc"] is None or not d["doc"]["active"])' 6; then pass "Terminal in front: skipped, no document attached"; else fail "Terminal not skipped: $(probe /state | jsonq '(d["status"]["frontmost"], d["status"]["skipped"], (d["doc"] or {}).get("app_name"), (d["doc"] or {}).get("active"))')"; fi
else
  echo "  ----  Terminal.app not running; the skip-list step was not exercised"
fi
summary
