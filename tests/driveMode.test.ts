import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import {
  DRIVE_HIDDEN_SURFACES,
  DRIVE_LABELS,
  commitDestructive,
  destructiveNeedsConfirmation,
  driveNextIndex,
  drivePreviousIndex,
  driveSkipSeconds,
  hiddenSurfaces,
  removalStep,
  visibleChrome,
} from "../src/driveMode.ts";

test("bil-läge döljer bibliotek, inställningar, import, Whisper och statistik", () => {
  assert.deepEqual(hiddenSurfaces(true), [
    "library",
    "settings",
    "import",
    "whisper",
    "statistics",
  ]);
  const chrome = visibleChrome(true);
  for (const surface of DRIVE_HIDDEN_SURFACES) {
    assert.equal(chrome[surface], false, surface);
  }
  assert.equal(chrome.seekSlider, false);
  assert.equal(chrome.drive, true);
  assert.equal(chrome.queue, true);

  const shown = visibleChrome(false);
  for (const surface of DRIVE_HIDDEN_SURFACES) {
    assert.equal(shown[surface], true, surface);
  }
  assert.equal(shown.seekSlider, true);
  assert.equal(shown.drive, false);
});

test("bil-lägets vy kopplar samma grindar och har inget sökreglage", () => {
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  const view = readFileSync(new URL("../src/DriveModeView.tsx", import.meta.url), "utf8");
  assert.match(app, /visibleChrome\(driveMode\)/);
  assert.match(app, /chrome\.library/);
  assert.match(app, /chrome\.settings/);
  assert.match(app, /chrome\.import/);
  assert.match(app, /chrome\.whisper/);
  assert.match(app, /chrome\.statistics/);
  assert.match(app, /chrome\.seekSlider/);
  assert.match(view, /hiddenSurfaces\(true\)/);
  assert.match(view, /removalStep\(/);
  assert.match(view, /DRIVE_LABELS/);
  assert.doesNotMatch(view, /type="range"/);
  assert.doesNotMatch(view, /Bibliotek|Inställningar|Importera|Whisper|Statistik/);
  const labels = Object.values(DRIVE_LABELS).join("\n");
  assert.match(labels, /Fortsätt där du var/);
  assert.match(labels, /Spela/);
  assert.match(labels, /Paus/);
  assert.doesNotMatch(labels, /Bibliotek|Inställningar|Importera|Whisper|Statistik/);
});

test("destruktiva åtgärder kräver bekräftelse", () => {
  assert.equal(destructiveNeedsConfirmation("remove-queue-item"), true);
  const armed = removalStep(null, 7, false);
  assert.equal(armed.remove, false);
  assert.equal(armed.pendingId, 7);
  assert.equal(commitDestructive(null, 7), false);
  assert.equal(commitDestructive(armed.pendingId, 8), false);

  const wrong = removalStep(armed.pendingId, 8, true);
  assert.equal(wrong.remove, false);
  assert.equal(wrong.pendingId, 7);

  const confirmed = removalStep(armed.pendingId, 7, true);
  assert.equal(confirmed.remove, true);
  assert.equal(confirmed.pendingId, null);

  const unarmed = removalStep(null, 7, true);
  assert.equal(unarmed.remove, false);
});

test("bil-läge tillåter bara ±15 och ±30 och hoppar inte till början", () => {
  assert.deepEqual(
    [-30, -15, 15, 30].map((delta) => driveSkipSeconds(delta)),
    [-30, -15, 15, 30],
  );
  assert.equal(driveSkipSeconds(0), null);
  assert.equal(driveSkipSeconds(5), null);
  assert.equal(driveSkipSeconds(45), null);
  assert.equal(drivePreviousIndex(0), null);
  assert.equal(drivePreviousIndex(-1), null);
  assert.equal(drivePreviousIndex(2), 1);
  assert.equal(driveNextIndex(0, 2), 1);
  assert.equal(driveNextIndex(1, 2), null);
  assert.equal(driveNextIndex(-1, 2), 0);
  assert.equal(driveNextIndex(0, 0), null);
});
